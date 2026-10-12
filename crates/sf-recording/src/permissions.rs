//! macOS privacy (TCC) checks for the three permissions a recording needs.
//!
//! - Screen Recording: ScreenCaptureKit refuses to list or capture content without it.
//! - Input Monitoring: the listen-only `CGEventTap` used for `mousemoves/mouseclicks/keystrokes`
//!   cannot be created without it.
//! - Microphone: CoreAudio input.
//!
//! `request_*` functions trigger the system prompt (first time) or no-op; the user may have
//! to enable the app in System Settings and relaunch. [`open_settings`] deep-links there.

/// Result of a permission check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionStatus {
    Granted,
    Denied,
    /// The user has not been asked yet (only distinguishable for the microphone).
    NotDetermined,
    /// Not applicable on this OS.
    Unsupported,
}

impl PermissionStatus {
    pub fn is_granted(self) -> bool {
        matches!(self, PermissionStatus::Granted)
    }
}

/// Which permission a [`open_settings`] call should show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    ScreenRecording,
    InputMonitoring,
    Microphone,
}

#[cfg(target_os = "macos")]
mod imp {
    use super::PermissionStatus;

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
        fn CGRequestScreenCaptureAccess() -> bool;
        fn CGPreflightListenEventAccess() -> bool;
        fn CGRequestListenEventAccess() -> bool;
    }

    #[link(name = "AVFoundation", kind = "framework")]
    extern "C" {}

    pub fn screen_recording() -> PermissionStatus {
        if unsafe { CGPreflightScreenCaptureAccess() } {
            return PermissionStatus::Granted;
        }
        // The preflight can report false although capture is allowed (e.g. after macOS updates
        // or when the grant was made for an earlier build). Ask ScreenCaptureKit directly: it
        // only returns displays when Screen Recording is really granted.
        let ok = std::panic::catch_unwind(|| {
            screencapturekit::shareable_content::SCShareableContent::create()
                .with_on_screen_windows_only(true)
                .get()
                .ok()
                .and_then(|c| c.snapshot())
                .is_some_and(|s| !s.displays.is_empty())
        })
        .unwrap_or(false);
        if ok { PermissionStatus::Granted } else { PermissionStatus::Denied }
    }

    pub fn request_screen_recording() -> PermissionStatus {
        if unsafe { CGRequestScreenCaptureAccess() } {
            PermissionStatus::Granted
        } else {
            PermissionStatus::Denied
        }
    }

    pub fn input_monitoring() -> PermissionStatus {
        if unsafe { CGPreflightListenEventAccess() } {
            PermissionStatus::Granted
        } else {
            PermissionStatus::Denied
        }
    }

    pub fn request_input_monitoring() -> PermissionStatus {
        if unsafe { CGRequestListenEventAccess() } {
            PermissionStatus::Granted
        } else {
            PermissionStatus::Denied
        }
    }

    fn av_capture_device() -> Option<&'static objc2::runtime::AnyClass> {
        objc2::runtime::AnyClass::get(c"AVCaptureDevice")
    }

    pub fn microphone() -> PermissionStatus {
        use objc2::msg_send;
        use objc2_foundation::NSString;
        let Some(cls) = av_capture_device() else { return PermissionStatus::Unsupported };
        let media = NSString::from_str("soun"); // AVMediaTypeAudio
        // AVAuthorizationStatus: 0 notDetermined, 1 restricted, 2 denied, 3 authorized
        let status: isize = unsafe { msg_send![cls, authorizationStatusForMediaType: &*media] };
        match status {
            3 => PermissionStatus::Granted,
            0 => PermissionStatus::NotDetermined,
            _ => PermissionStatus::Denied,
        }
    }

    /// Shows the microphone prompt if not determined; returns the status known right now
    /// (the prompt completes asynchronously).
    pub fn request_microphone() -> PermissionStatus {
        use block2::RcBlock;
        use objc2::msg_send;
        use objc2::runtime::Bool;
        use objc2_foundation::NSString;
        let status = microphone();
        if status != PermissionStatus::NotDetermined {
            return status;
        }
        let Some(cls) = av_capture_device() else { return PermissionStatus::Unsupported };
        let media = NSString::from_str("soun");
        let block = RcBlock::new(|_granted: Bool| {});
        let _: () = unsafe { msg_send![cls, requestAccessForMediaType: &*media, completionHandler: &*block] };
        PermissionStatus::NotDetermined
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::PermissionStatus;
    pub fn screen_recording() -> PermissionStatus {
        PermissionStatus::Unsupported
    }
    pub fn request_screen_recording() -> PermissionStatus {
        PermissionStatus::Unsupported
    }
    pub fn input_monitoring() -> PermissionStatus {
        PermissionStatus::Unsupported
    }
    pub fn request_input_monitoring() -> PermissionStatus {
        PermissionStatus::Unsupported
    }
    pub fn microphone() -> PermissionStatus {
        PermissionStatus::Unsupported
    }
    pub fn request_microphone() -> PermissionStatus {
        PermissionStatus::Unsupported
    }
}

/// Screen Recording permission (`CGPreflightScreenCaptureAccess`).
pub fn screen_recording() -> PermissionStatus {
    imp::screen_recording()
}
/// Input Monitoring permission (`CGPreflightListenEventAccess`).
pub fn input_monitoring() -> PermissionStatus {
    imp::input_monitoring()
}
/// Microphone permission (`AVCaptureDevice.authorizationStatus(for: .audio)`).
pub fn microphone() -> PermissionStatus {
    imp::microphone()
}
/// Prompt for Screen Recording (adds the app to the list; the user enables it and relaunches).
pub fn request_screen_recording() -> PermissionStatus {
    imp::request_screen_recording()
}
/// Prompt for Input Monitoring.
pub fn request_input_monitoring() -> PermissionStatus {
    imp::request_input_monitoring()
}
/// Prompt for the microphone.
pub fn request_microphone() -> PermissionStatus {
    imp::request_microphone()
}

/// Open System Settings on the privacy pane for `permission`.
pub fn open_settings(permission: Permission) -> std::io::Result<()> {
    let anchor = match permission {
        Permission::ScreenRecording => "Privacy_ScreenCapture",
        Permission::InputMonitoring => "Privacy_ListenEvent",
        Permission::Microphone => "Privacy_Microphone",
    };
    let url = format!("x-apple.systempreferences:com.apple.preference.security?{anchor}");
    if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(url).status().map(|_| ())
    } else {
        Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "macOS only"))
    }
}
