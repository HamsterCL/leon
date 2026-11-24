//! Nintendo Switch Pro Controller, read straight from its HID interface.
//!
//! Windows lists the pad over Bluetooth but delivers no input to XInput,
//! `winmm` or `Windows.Gaming.Input`: the controller reports only after the
//! program has set its report mode, which is what Steam does for it. We do
//! the same, then read the standard full reports (id `0x30`) on a thread of
//! our own and keep the latest one for [`super::Source::pad`].
//!
//! Layout of a `0x30` report (community notes on the controller's protocol,
//! checked against this pad's own reports):
//!
//! | byte | content |
//! |------|---------|
//! | 0 | report id, `0x30` |
//! | 3 | Y `01`, X `02`, B `04`, A `08`, R `40`, ZR `80` |
//! | 4 | Minus `01`, Plus `02`, R stick `04`, L stick `08`, Home `10`, Capture `20` |
//! | 5 | Down `01`, Up `02`, Right `04`, Left `08`, L `40`, ZL `80` |
//! | 6-8 | left stick, two 12-bit values (x, then y) |
//! | 9-11 | right stick, the same |
//!
//! Buttons go by position, not by label: the Switch's A and B (and X and
//! Y) are swapped against an Xbox pad's, so the button at the bottom of the
//! diamond is the title's `A` on either. ZL and ZR are switches, not
//! triggers: they read 0 or 1.

use super::{
    A, B, DOWN, LEFT, LEFT_SHOULDER, LEFT_STICK, MENU, PadState, RIGHT, RIGHT_SHOULDER,
    RIGHT_STICK, UP, VIEW, X, Y,
};

/// The id of a standard full input report.
const FULL: u8 = 0x30;
/// Shortest report that holds the buttons and both sticks.
const LEN: usize = 12;
/// A stick at rest reads about 2048; full travel is some 1500 either way.
/// The pad stores its own calibration in flash; these are its typical values.
const STICK_CENTRE: f32 = 2048.0;
const STICK_RANGE: f32 = 1500.0;

/// (byte, mask, the title's button) for every digital input that maps.
const BUTTONS: [(usize, u8, u32); 14] = [
    (3, 0x04, A),              // B, at the bottom
    (3, 0x08, B),              // A, at the right
    (3, 0x01, X),              // Y, at the left
    (3, 0x02, Y),              // X, at the top
    (3, 0x40, RIGHT_SHOULDER), // R
    (4, 0x01, VIEW),           // Minus
    (4, 0x02, MENU),           // Plus
    (4, 0x04, RIGHT_STICK),
    (4, 0x08, LEFT_STICK),
    (5, 0x01, DOWN),
    (5, 0x02, UP),
    (5, 0x04, RIGHT),
    (5, 0x08, LEFT),
    (5, 0x40, LEFT_SHOULDER), // L
];

/// A 12-bit stick axis, centred and scaled to -1.0..=1.0.
fn axis(raw: u16) -> f32 {
    ((f32::from(raw) - STICK_CENTRE) / STICK_RANGE).clamp(-1.0, 1.0)
}

/// The two 12-bit values packed in three bytes.
fn stick(b: &[u8]) -> (u16, u16) {
    (
        u16::from(b[0]) | (u16::from(b[1] & 0x0f) << 8),
        u16::from(b[1] >> 4) | (u16::from(b[2]) << 4),
    )
}

/// The pad's state from one input report, `None` for any other report.
pub(super) fn parse(report: &[u8]) -> Option<PadState> {
    if report.len() < LEN || report[0] != FULL {
        return None;
    }
    let (lx, ly) = stick(&report[6..9]);
    let (rx, ry) = stick(&report[9..12]);
    let switch = |byte: usize, mask: u8| if report[byte] & mask != 0 { 1.0 } else { 0.0 };
    Some(PadState {
        buttons: BUTTONS
            .iter()
            .filter(|&&(byte, mask, _)| report[byte] & mask != 0)
            .fold(0, |all, &(_, _, bit)| all | bit),
        left_trigger: switch(5, 0x80),
        right_trigger: switch(3, 0x80),
        left_x: axis(lx),
        // The pad counts up; so does the title.
        left_y: axis(ly),
        right_x: axis(rx),
        right_y: axis(ry),
    })
}

#[cfg(windows)]
pub(super) use host::SwitchPro;

#[cfg(windows)]
mod host {
    use std::sync::{Arc, Mutex, PoisonError};
    use std::time::Duration;

    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
        DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, SP_DEVICE_INTERFACE_DATA,
        SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW,
        SetupDiGetDeviceInterfaceDetailW,
    };
    use windows_sys::Win32::Devices::HumanInterfaceDevice::{
        HIDD_ATTRIBUTES, HidD_GetAttributes, HidD_GetHidGuid,
    };
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, ReadFile, WriteFile,
    };

    use super::{PadState, parse};

    const VENDOR_NINTENDO: u16 = 0x057E;
    const PRO_CONTROLLER: u16 = 0x2009;
    /// Look for the pad this often while it is away.
    const RETRY: Duration = Duration::from_secs(2);
    /// Bytes of an output report to the pad (`HidP_GetCaps`: 49).
    const OUTPUT_LEN: usize = 49;
    /// Longest input report we read; the pad's own is 362 bytes through HID.
    const READ_LEN: usize = 362;

    /// The latest report of a Pro Controller, `None` while none is there.
    /// Reading happens on a thread of its own: `ReadFile` blocks.
    pub(in super::super) struct SwitchPro {
        state: Arc<Mutex<Option<PadState>>>,
    }

    impl Default for SwitchPro {
        fn default() -> Self {
            let state = Arc::new(Mutex::new(None));
            let shared = Arc::clone(&state);
            // Detached: it ends with the process.
            let spawned = std::thread::Builder::new()
                .name("switch-pro".into())
                .spawn(move || read_forever(&shared));
            if let Err(err) = spawned {
                tracing::warn!(%err, "no thread for the Switch Pro Controller");
            }
            Self { state }
        }
    }

    impl SwitchPro {
        pub(in super::super) fn state(&self) -> Option<PadState> {
            *self.state.lock().unwrap_or_else(PoisonError::into_inner)
        }
    }

    fn set(state: &Mutex<Option<PadState>>, value: Option<PadState>) {
        *state.lock().unwrap_or_else(PoisonError::into_inner) = value;
    }

    /// Finds the pad, reads it until it goes away, and starts over.
    fn read_forever(state: &Mutex<Option<PadState>>) {
        loop {
            if let Some(handle) = open() {
                tracing::info!("Switch Pro Controller connected");
                read(handle, state);
                // SAFETY: a handle `open` made, closed once.
                unsafe { CloseHandle(handle) };
                set(state, None);
                tracing::info!("Switch Pro Controller gone");
            }
            std::thread::sleep(RETRY);
        }
    }

    /// Reads reports until the handle fails.
    fn read(handle: HANDLE, state: &Mutex<Option<PadState>>) {
        // Standard full reports: the pad sends the simple ones, with no
        // sticks past the first, until it is told. Subcommand 0x03.
        let mut output = [0_u8; OUTPUT_LEN];
        output[0] = 0x01; // output report: rumble and subcommand
        output[2..6].copy_from_slice(&[0x00, 0x01, 0x40, 0x40]); // neutral rumble
        output[6..10].copy_from_slice(&[0x00, 0x01, 0x40, 0x40]);
        output[10] = 0x03; // set the input report mode
        output[11] = 0x30; // standard full
        let mut written = 0_u32;
        // SAFETY: the buffer and the count outlive the call; no overlap.
        let sent = unsafe {
            WriteFile(
                handle,
                output.as_ptr(),
                OUTPUT_LEN as u32,
                &mut written,
                std::ptr::null_mut(),
            )
        };
        if sent == 0 {
            tracing::warn!("Switch Pro Controller refused its report mode; reading as it is");
        }
        let mut report = [0_u8; READ_LEN];
        loop {
            let mut got = 0_u32;
            // SAFETY: room for `READ_LEN` bytes; no overlap, so it blocks.
            let ok = unsafe {
                ReadFile(
                    handle,
                    report.as_mut_ptr(),
                    READ_LEN as u32,
                    &mut got,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                return;
            }
            if let Some(pad) = parse(&report[..got as usize]) {
                set(state, Some(pad));
            }
        }
    }

    /// The first Pro Controller's HID interface, opened for reading and writing.
    fn open() -> Option<HANDLE> {
        // SAFETY: plain SetupAPI and HID calls on buffers sized for them; the
        // device list is destroyed on every path.
        unsafe {
            let mut guid = std::mem::zeroed();
            HidD_GetHidGuid(&mut guid);
            let set = SetupDiGetClassDevsW(
                &guid,
                std::ptr::null(),
                std::ptr::null_mut(),
                DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
            );
            if set == -1 {
                return None;
            }
            let mut found = None;
            for index in 0.. {
                let mut interface: SP_DEVICE_INTERFACE_DATA = std::mem::zeroed();
                interface.cbSize = size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
                if SetupDiEnumDeviceInterfaces(set, std::ptr::null(), &guid, index, &mut interface)
                    == 0
                {
                    break;
                }
                let Some(path) = path_of(set, &interface) else {
                    continue;
                };
                if let Some(handle) = open_pro(&path) {
                    found = Some(handle);
                    break;
                }
            }
            SetupDiDestroyDeviceInfoList(set);
            found
        }
    }

    /// The device path of one HID interface, NUL-terminated UTF-16.
    ///
    /// # Safety
    /// `set` and `interface` come from the enumeration just made.
    unsafe fn path_of(set: isize, interface: &SP_DEVICE_INTERFACE_DATA) -> Option<Vec<u16>> {
        // SAFETY: sizes come from the first call; the buffer is 8-aligned
        // by being made of u64s, as the detail's header needs.
        unsafe {
            let mut need = 0_u32;
            SetupDiGetDeviceInterfaceDetailW(
                set,
                interface,
                std::ptr::null_mut(),
                0,
                &mut need,
                std::ptr::null_mut(),
            );
            if need < 6 {
                return None;
            }
            let mut buffer = vec![0_u64; (need as usize).div_ceil(8)];
            let detail = buffer.as_mut_ptr().cast::<u8>();
            // cbSize of SP_DEVICE_INTERFACE_DETAIL_DATA_W: 8 on 64-bit.
            detail
                .cast::<u32>()
                .write(if size_of::<usize>() == 8 { 8 } else { 6 });
            if SetupDiGetDeviceInterfaceDetailW(
                set,
                interface,
                detail.cast(),
                need,
                &mut need,
                std::ptr::null_mut(),
            ) == 0
            {
                return None;
            }
            let chars = detail.add(4).cast::<u16>();
            let max = (need as usize).saturating_sub(4) / 2;
            let len = (0..max).take_while(|&i| *chars.add(i) != 0).count();
            let mut path: Vec<u16> = std::slice::from_raw_parts(chars, len).to_vec();
            path.push(0);
            Some(path)
        }
    }

    /// Opens `path` when it is a Pro Controller.
    fn open_pro(path: &[u16]) -> Option<HANDLE> {
        // SAFETY: a NUL-terminated path; the handle is closed unless returned.
        unsafe {
            let handle = CreateFileW(
                path.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            );
            if handle == INVALID_HANDLE_VALUE {
                return None;
            }
            let mut attributes: HIDD_ATTRIBUTES = std::mem::zeroed();
            attributes.Size = size_of::<HIDD_ATTRIBUTES>() as u32;
            let pro = HidD_GetAttributes(handle, &mut attributes)
                && attributes.VendorID == VENDOR_NINTENDO
                && attributes.ProductID == PRO_CONTROLLER;
            if pro {
                Some(handle)
            } else {
                CloseHandle(handle);
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A report with these button bytes and both sticks at rest.
    fn report(right: u8, shared: u8, left: u8) -> [u8; 12] {
        [
            FULL, 0x92, 0x60, right, shared, left, 0x00, 0x08, 0x80, 0x00, 0x08, 0x80,
        ]
    }

    #[test]
    fn a_pad_at_rest_reads_nothing() {
        // The first report this pad sent, byte for byte.
        let rest = [
            0x30, 0x92, 0x60, 0x00, 0x80, 0x00, 0x00, 0x08, 0x80, 0x00, 0x08, 0x80,
        ];
        assert_eq!(parse(&rest), Some(PadState::default()));
    }

    #[test]
    fn buttons_go_by_position_not_by_label() {
        let pad = |r, s, l| parse(&report(r, s, l)).unwrap().buttons;
        assert_eq!(pad(0x04, 0, 0), A, "the Switch's B is at the bottom");
        assert_eq!(pad(0x08, 0, 0), B, "its A is at the right");
        assert_eq!(pad(0x01, 0, 0), X, "its Y is at the left");
        assert_eq!(pad(0x02, 0, 0), Y, "its X is at the top");
        assert_eq!(pad(0x40, 0, 0), RIGHT_SHOULDER);
        assert_eq!(pad(0, 0x01, 0), VIEW);
        assert_eq!(pad(0, 0x02, 0), MENU);
        assert_eq!(pad(0, 0x04, 0), RIGHT_STICK);
        assert_eq!(pad(0, 0x08, 0), LEFT_STICK);
        assert_eq!(pad(0, 0, 0x01), DOWN);
        assert_eq!(pad(0, 0, 0x02), UP);
        assert_eq!(pad(0, 0, 0x04), RIGHT);
        assert_eq!(pad(0, 0, 0x08), LEFT);
        assert_eq!(pad(0, 0, 0x40), LEFT_SHOULDER);
        // Home, Capture, the charging grip and the side rails are ignored.
        assert_eq!(pad(0x30, 0xb0, 0x30), 0);
    }

    #[test]
    fn several_buttons_add_up() {
        let both = parse(&report(0x04 | 0x08, 0x02, 0x02)).unwrap().buttons;
        assert_eq!(both, A | B | MENU | UP);
    }

    #[test]
    fn zl_and_zr_are_switches() {
        let pad = parse(&report(0x80, 0, 0x80)).unwrap();
        assert_eq!((pad.left_trigger, pad.right_trigger), (1.0, 1.0));
        let none = parse(&report(0, 0, 0)).unwrap();
        assert_eq!((none.left_trigger, none.right_trigger), (0.0, 0.0));
        assert_eq!(pad.buttons, 0, "they press no button of their own");
    }

    #[test]
    fn sticks_unpack_twelve_bits_each() {
        // x = 0xFFF (full right), y = 0x000 (full down), packed as the pad does.
        let mut r = report(0, 0, 0);
        r[6..9].copy_from_slice(&[0xff, 0x0f, 0x00]);
        let pad = parse(&r).unwrap();
        assert_eq!((pad.left_x, pad.left_y), (1.0, -1.0));
        // y = 0xFFF is full up; x = 0x000 full left.
        r[6..9].copy_from_slice(&[0x00, 0xf0, 0xff]);
        let pad = parse(&r).unwrap();
        assert_eq!((pad.left_x, pad.left_y), (-1.0, 1.0));
        // The right stick is read from its own bytes.
        r[6..9].copy_from_slice(&[0x00, 0x08, 0x80]);
        r[9..12].copy_from_slice(&[0xff, 0x0f, 0x00]);
        let pad = parse(&r).unwrap();
        assert_eq!((pad.left_x, pad.right_x, pad.right_y), (0.0, 1.0, -1.0));
    }

    #[test]
    fn other_reports_are_not_the_pad() {
        let mut r = report(0, 0, 0);
        r[0] = 0x3f;
        assert_eq!(parse(&r), None);
        assert_eq!(parse(&r[..11]), None);
        assert_eq!(parse(&[]), None);
    }
}
