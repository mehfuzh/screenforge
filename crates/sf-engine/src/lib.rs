//! Pure, GPU-independent editing logic shared by the preview and the exporter.
//!
//! * [`timeline`] – maps output time ↔ source time through the scene's slices.
//! * [`spring`]   – react-spring compatible damped spring integrator used for the virtual
//!                  camera (zoom/pan) and cursor smoothing.
//! * [`cursor`]   – samples the raw mouse log at any source time, removes shake, smooths with
//!                  a spring, derives velocity based tilt.
//! * [`camera`]   – resolves the visible source rect (viewport) for an output time from the
//!                  scene's zooms.
//! * [`stage`]    – stage geometry: canvas aspect, padding, zoomed screen rect.
//! * [`autozoom`] – automatic zoom ranges from clicks.
//! * [`shortcuts`] – keyboard shortcut labels from key-downs.

pub mod audio;
pub mod autozoom;
pub mod camera;
pub mod captions;
pub mod cursor;
pub mod device;
pub mod lanes;
pub mod spring;
pub mod shortcuts;
pub mod stage;
pub mod timeline;

pub use glam::{DVec2, Vec2};
