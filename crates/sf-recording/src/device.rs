//! iPhone and iPad screens over USB, as QuickTime records them: once a process opts in through
//! CoreMediaIO, each connected (unlocked, trusted) device appears as an AVFoundation capture
//! device whose video is its screen. Frames land in a [`FrameStore`](crate::repeater::FrameStore)
//! like the display's, so the same repeater writes the `display` channel.
//!
//! There is no pointer or keyboard on the device: recordings have no input logs, so no click
//! zooms or cursor. The frame size comes from the device's first frame; if the device is turned
//! while recording, later frames are fitted into that size (letterboxed) rather than dropped.

/// A connected iPhone or iPad.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    /// AVFoundation `uniqueID`.
    pub id: String,
    pub name: String,
}

/// Fit a `sw`×`sh` BGRA frame (rows `stride` bytes apart) into `dw`×`dh`, centred on black
/// (nearest neighbour; only used after the device was turned).
pub fn fit_bgra(src: &[u8], sw: usize, sh: usize, stride: usize, dw: usize, dh: usize) -> Vec<u8> {
    let mut out = vec![0u8; dw * dh * 4];
    for px in out.chunks_exact_mut(4) {
        px[3] = 255;
    }
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 {
        return out;
    }
    let scale = (dw as f64 / sw as f64).min(dh as f64 / sh as f64);
    let (fw, fh) = (((sw as f64 * scale) as usize).clamp(1, dw), ((sh as f64 * scale) as usize).clamp(1, dh));
    let (ox, oy) = ((dw - fw) / 2, (dh - fh) / 2);
    for y in 0..fh {
        let sy = ((y as f64 + 0.5) / scale) as usize;
        let row = &src[sy.min(sh - 1) * stride..];
        let dst = &mut out[((oy + y) * dw + ox) * 4..((oy + y) * dw + ox + fw) * 4];
        for (x, px) in dst.chunks_exact_mut(4).enumerate() {
            let sx = (((x as f64 + 0.5) / scale) as usize).min(sw - 1) * 4;
            px.copy_from_slice(&row[sx..sx + 4]);
        }
    }
    out
}

#[cfg(target_os = "macos")]
pub use mac::{devices, DeviceCapture};

/// No device capture outside macOS.
#[cfg(not(target_os = "macos"))]
pub fn devices() -> Vec<DeviceInfo> {
    Vec::new()
}

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::c_void;
    use std::sync::{Arc, OnceLock};
    use std::time::{Duration, Instant};

    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject, Bool, NSObject};
    use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass};
    use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString};
    use parking_lot::Mutex;

    use super::{fit_bgra, DeviceInfo};
    use crate::repeater::FrameStore;
    use crate::{Error, Result};

    #[repr(C)]
    struct CmioPropertyAddress {
        selector: u32,
        scope: u32,
        element: u32,
    }

    #[link(name = "CoreMediaIO", kind = "framework")]
    extern "C" {
        fn CMIOObjectSetPropertyData(object: u32, address: *const CmioPropertyAddress, qualifier_size: u32, qualifier: *const c_void, data_size: u32, data: *const c_void) -> i32;
    }
    #[link(name = "CoreMedia", kind = "framework")]
    extern "C" {
        fn CMSampleBufferGetImageBuffer(sample: *mut c_void) -> *mut c_void;
    }
    #[link(name = "CoreVideo", kind = "framework")]
    extern "C" {
        fn CVPixelBufferLockBaseAddress(pb: *mut c_void, flags: u64) -> i32;
        fn CVPixelBufferUnlockBaseAddress(pb: *mut c_void, flags: u64) -> i32;
        fn CVPixelBufferGetWidth(pb: *mut c_void) -> usize;
        fn CVPixelBufferGetHeight(pb: *mut c_void) -> usize;
        fn CVPixelBufferGetBytesPerRow(pb: *mut c_void) -> usize;
        fn CVPixelBufferGetBaseAddress(pb: *mut c_void) -> *mut u8;
    }
    #[link(name = "AVFoundation", kind = "framework")]
    extern "C" {}
    extern "C" {
        fn dispatch_queue_create(label: *const std::ffi::c_char, attr: *const c_void) -> *mut AnyObject;
    }

    const fn fourcc(s: &[u8; 4]) -> u32 {
        u32::from_be_bytes(*s)
    }
    /// `kCMIOObjectSystemObject`
    const CMIO_SYSTEM_OBJECT: u32 = 1;
    /// `kCVPixelFormatType_32BGRA`
    const BGRA: u32 = fourcc(b"BGRA");
    /// `AVMediaTypeMuxed`: iOS screens are muxed (video + audio) devices.
    const MUXED: &str = "muxx";
    /// Model of the screen capture devices CoreMediaIO adds for iPhones and iPads.
    const IOS_MODEL: &str = "iOS Device";

    /// Let this process see iOS screens as capture devices (`kCMIOHardwarePropertyAllowScreenCaptureDevices`).
    fn allow_screen_devices() {
        static DONE: OnceLock<()> = OnceLock::new();
        DONE.get_or_init(|| {
            let addr = CmioPropertyAddress { selector: fourcc(b"yes "), scope: fourcc(b"glob"), element: 0 };
            let allow: u32 = 1;
            unsafe { CMIOObjectSetPropertyData(CMIO_SYSTEM_OBJECT, &addr, 0, std::ptr::null(), 4, (&allow as *const u32).cast()) };
        });
    }

    fn class(name: &std::ffi::CStr) -> Result<&'static AnyClass> {
        AnyClass::get(name).ok_or_else(|| Error::Capture(format!("{} is not available", name.to_string_lossy())))
    }

    fn ns_string(obj: &AnyObject, selector: &str) -> String {
        let s: Option<Retained<NSString>> = match selector {
            "uniqueID" => unsafe { msg_send![obj, uniqueID] },
            "localizedName" => unsafe { msg_send![obj, localizedName] },
            _ => unsafe { msg_send![obj, modelID] },
        };
        s.map(|s| s.to_string()).unwrap_or_default()
    }

    /// Muxed devices from the discovery session (external devices) and from the older
    /// `devicesWithMediaType:`, so either API going away leaves the other; iOS screens only.
    fn ios_devices() -> Vec<Retained<AnyObject>> {
        let media = NSString::from_str(MUXED);
        let mut all: Vec<Retained<AnyObject>> = vec![];
        if let Ok(cls) = class(c"AVCaptureDeviceDiscoverySession") {
            let types = NSArray::from_retained_slice(&[NSString::from_str("AVCaptureDeviceTypeExternal"), NSString::from_str("AVCaptureDeviceTypeExternalUnknown")]);
            // position 0 = unspecified
            let session: Option<Retained<AnyObject>> = unsafe { msg_send![cls, discoverySessionWithDeviceTypes: &*types, mediaType: &*media, position: 0isize] };
            if let Some(s) = session {
                let list: Option<Retained<NSArray<AnyObject>>> = unsafe { msg_send![&*s, devices] };
                all.extend(list.map(|l| l.to_vec()).unwrap_or_default());
            }
        }
        if let Ok(cls) = class(c"AVCaptureDevice") {
            let list: Option<Retained<NSArray<AnyObject>>> = unsafe { msg_send![cls, devicesWithMediaType: &*media] };
            for d in list.map(|l| l.to_vec()).unwrap_or_default() {
                let id = ns_string(&d, "uniqueID");
                if !all.iter().any(|x| ns_string(x, "uniqueID") == id) {
                    all.push(d);
                }
            }
        }
        all.into_iter().filter(|d| ns_string(d, "modelID") == IOS_MODEL).collect()
    }

    /// iPhones and iPads connected by cable (unlocked and trusting this Mac). The first call
    /// opts in and waits up to `wait` for devices to appear: they show up a moment later.
    pub fn devices_within(wait: Duration) -> Vec<super::DeviceInfo> {
        allow_screen_devices();
        let until = Instant::now() + wait;
        loop {
            let found = ios_devices();
            if !found.is_empty() || Instant::now() >= until {
                return found.iter().map(|d| DeviceInfo { id: ns_string(d, "uniqueID"), name: ns_string(d, "localizedName") }).collect();
            }
            std::thread::sleep(Duration::from_millis(150));
        }
    }

    /// Connected iPhones and iPads (waits up to 2 s on first use).
    pub fn devices() -> Vec<DeviceInfo> {
        devices_within(Duration::from_secs(2))
    }

    /// What the frame callback writes into.
    #[derive(Default)]
    pub struct Shared {
        /// Created with the first frame's size.
        store: OnceLock<Arc<FrameStore>>,
        /// Frames that arrived at another size (the device was turned) and were fitted.
        pub turned: std::sync::atomic::AtomicU64,
        first_size: Mutex<Option<(usize, usize)>>,
    }

    impl Shared {
        fn on_frame(&self, sample: *mut c_void) {
            unsafe {
                let pb = CMSampleBufferGetImageBuffer(sample);
                if pb.is_null() || CVPixelBufferLockBaseAddress(pb, 1) != 0 {
                    return;
                }
                let (w, h, stride, base) = (CVPixelBufferGetWidth(pb), CVPixelBufferGetHeight(pb), CVPixelBufferGetBytesPerRow(pb), CVPixelBufferGetBaseAddress(pb));
                if !base.is_null() && w > 0 && h > 0 {
                    let src = std::slice::from_raw_parts(base, stride * h);
                    let store = self.store.get_or_init(|| {
                        *self.first_size.lock() = Some((w, h));
                        Arc::new(FrameStore::new(w, h))
                    });
                    let frame = if (w, h) == (store.width, store.height) {
                        let row = w * 4;
                        let mut buf = Vec::with_capacity(row * h);
                        for y in 0..h {
                            buf.extend_from_slice(&src[y * stride..y * stride + row]);
                        }
                        buf
                    } else {
                        self.turned.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        fit_bgra(src, w, h, stride, store.width, store.height)
                    };
                    store.put(frame);
                }
                CVPixelBufferUnlockBaseAddress(pb, 1);
            }
        }
    }

    define_class!(
        // SAFETY: NSObject has no subclassing requirements; the class adds no Drop.
        #[unsafe(super(NSObject))]
        #[name = "SFDeviceFrameDelegate"]
        #[ivars = Arc<Shared>]
        struct FrameDelegate;

        impl FrameDelegate {
            // AVCaptureVideoDataOutputSampleBufferDelegate (called on the capture queue)
            #[unsafe(method(captureOutput:didOutputSampleBuffer:fromConnection:))]
            fn did_output(&self, _output: *mut AnyObject, sample: *mut c_void, _connection: *mut AnyObject) {
                self.ivars().on_frame(sample);
            }
        }
    );

    impl FrameDelegate {
        fn new(shared: Arc<Shared>) -> Retained<Self> {
            let this = Self::alloc().set_ivars(shared);
            unsafe { msg_send![super(this), init] }
        }
    }

    /// A running capture of one device's screen.
    pub struct DeviceCapture {
        session: Retained<AnyObject>,
        _delegate: Retained<FrameDelegate>,
        pub shared: Arc<Shared>,
        pub name: String,
    }

    // The session is only started and stopped (thread-safe in AVFoundation); frames arrive on
    // its own queue.
    unsafe impl Send for DeviceCapture {}
    unsafe impl Sync for DeviceCapture {}

    impl DeviceCapture {
        /// Open device `id` and wait for its first frame (the store then has the frame size).
        pub fn start(id: &str) -> Result<Self> {
            allow_screen_devices();
            let device = {
                let until = Instant::now() + Duration::from_secs(3);
                loop {
                    if let Some(d) = ios_devices().into_iter().find(|d| ns_string(d, "uniqueID") == id) {
                        break d;
                    }
                    if Instant::now() >= until {
                        return Err(Error::Capture("The iPhone or iPad is not connected any more. Connect it with a cable, unlock it and try again.".into()));
                    }
                    std::thread::sleep(Duration::from_millis(150));
                }
            };
            let name = ns_string(&device, "localizedName");
            unsafe {
                let session: Retained<AnyObject> = msg_send![class(c"AVCaptureSession")?, new];
                let mut err: *mut AnyObject = std::ptr::null_mut();
                let input: Option<Retained<AnyObject>> = msg_send![class(c"AVCaptureDeviceInput")?, deviceInputWithDevice: &*device, error: &mut err];
                let input = input.ok_or_else(|| Error::Capture(format!("could not open {name}: unlock it and tap Trust if it asks")))?;
                let can_in: Bool = msg_send![&*session, canAddInput: &*input];
                if !can_in.as_bool() {
                    return Err(Error::Capture(format!("{name} is in use by another app (QuickTime?)")));
                }
                let _: () = msg_send![&*session, addInput: &*input];

                let output: Retained<AnyObject> = msg_send![class(c"AVCaptureVideoDataOutput")?, new];
                let key = NSString::from_str("PixelFormatType"); // kCVPixelBufferPixelFormatTypeKey
                let value = NSNumber::new_u32(BGRA);
                let settings = NSDictionary::<NSString, AnyObject>::from_slices(&[&*key], &[&*value as &AnyObject]);
                let _: () = msg_send![&*output, setVideoSettings: &*settings];
                let _: () = msg_send![&*output, setAlwaysDiscardsLateVideoFrames: Bool::YES];
                let shared = Arc::new(Shared::default());
                let delegate = FrameDelegate::new(shared.clone());
                let queue = dispatch_queue_create(c"studio.screenforge.device".as_ptr(), std::ptr::null());
                let _: () = msg_send![&*output, setSampleBufferDelegate: &*delegate, queue: queue];
                let can_out: Bool = msg_send![&*session, canAddOutput: &*output];
                if !can_out.as_bool() {
                    return Err(Error::Capture(format!("cannot read video from {name}")));
                }
                let _: () = msg_send![&*session, addOutput: &*output];
                let _: () = msg_send![&*session, startRunning];

                let capture = DeviceCapture { session, _delegate: delegate, shared, name };
                let until = Instant::now() + Duration::from_secs(6);
                while capture.shared.store.get().is_none_or(|s| s.received.load(std::sync::atomic::Ordering::Relaxed) == 0) {
                    if Instant::now() >= until {
                        capture.stop();
                        return Err(Error::Capture(format!("{} sent no picture. Unlock it and keep the screen on.", capture.name)));
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Ok(capture)
            }
        }

        /// The frames, sized like the device's screen when capture started.
        pub(crate) fn store(&self) -> Arc<FrameStore> {
            self.shared.store.get().expect("started").clone()
        }

        pub fn stop(&self) {
            unsafe {
                let _: () = msg_send![&*self.session, stopRunning];
            }
        }
    }

    impl Drop for DeviceCapture {
        fn drop(&mut self) {
            self.stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fit_bgra;

    #[test]
    fn a_turned_frame_is_fitted_on_black() {
        // a 4×2 landscape frame (left half red, right half blue) into a 2×4 portrait canvas
        let mut src = vec![];
        for _ in 0..2 {
            for x in 0..4 {
                src.extend_from_slice(if x < 2 { &[0, 0, 255, 255] } else { &[255, 0, 0, 255] });
            }
        }
        let out = fit_bgra(&src, 4, 2, 16, 2, 4);
        let px = |x: usize, y: usize| &out[(y * 2 + x) * 4..(y * 2 + x) * 4 + 4];
        // scaled to 2×1 in the middle rows; black above and below
        assert_eq!(px(0, 0), &[0, 0, 0, 255]);
        assert_eq!(px(0, 1).len(), 4);
        assert_eq!(px(0, 3), &[0, 0, 0, 255]);
        let middle: Vec<&[u8]> = (1..3).flat_map(|y| (0..2).map(move |x| (x, y))).map(|(x, y)| px(x, y)).filter(|p| p != &[0, 0, 0, 255]).collect();
        assert!(!middle.is_empty() && middle.iter().any(|p| p[2] == 255) && middle.iter().any(|p| p[0] == 255));
    }

    #[test]
    fn same_aspect_scales_without_bars() {
        let src = vec![9u8; 4 * 4 * 4];
        let out = fit_bgra(&src, 4, 4, 16, 2, 2);
        assert!(out.chunks(4).all(|p| p == [9, 9, 9, 9]));
    }
}
