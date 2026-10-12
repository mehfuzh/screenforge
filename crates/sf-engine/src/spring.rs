//! Damped spring integrator matching Screen Studio's (react-spring style) `SpringConfig`
//! `{stiffness, damping, mass, clamp, precision}`.
//!
//! Semi-implicit Euler with 1 ms sub-steps. `clamp` raises damping to critical damping when
//! the spring is heading toward the target faster than a critically damped spring would, so
//! it never overshoots. `precision` is scaled by the distance of the last target change
//! (`max(1, |from - to|) * precision`), which is what makes the same config work for values
//! in pixels and in 0..1 units.

use sf_project::schema::common::Spring;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpringState {
    pub position: f64,
    pub velocity: f64,
}

impl SpringState {
    pub const fn at(position: f64) -> Self {
        SpringState { position, velocity: 0.0 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpringParams {
    pub stiffness: f64,
    pub damping: f64,
    pub mass: f64,
    pub clamp: bool,
    pub precision: f64,
}

impl SpringParams {
    /// Screen Studio's default for both the cursor and the screen.
    pub const DEFAULT: SpringParams = SpringParams { stiffness: 125.0, damping: 12.0, mass: 1.5, clamp: true, precision: 0.002 };
    /// `mass: 0` → snap instantly.
    pub const INSTANT: SpringParams = SpringParams { stiffness: 1.0, damping: 1.0, mass: 0.0, clamp: false, precision: 0.002 };

    // UI presets ("Animations" sidebar)
    pub const SCREEN_FOCUSED: SpringParams = SpringParams { stiffness: 250.0, damping: 40.0, mass: 2.25, clamp: true, precision: 0.002 };
    pub const SCREEN_SMOOTH: SpringParams = SpringParams { stiffness: 170.0, damping: 50.0, mass: 3.0, clamp: true, precision: 0.002 };
    pub const CURSOR_SMOOTH: SpringParams = SpringParams { stiffness: 470.0, damping: 70.0, mass: 3.0, clamp: true, precision: 0.002 };
    pub const CURSOR_MEDIUM: SpringParams = SpringParams { stiffness: 340.0, damping: 60.0, mass: 3.0, clamp: true, precision: 0.002 };
    pub const CURSOR_RAPID: SpringParams = SpringParams { stiffness: 530.0, damping: 40.0, mass: 1.0, clamp: true, precision: 0.002 };

    pub fn is_instant(&self) -> bool {
        self.mass <= 0.0
    }

    /// Advance `(x, v)` toward `target` by `dt_ms`. `precision` must already be scaled
    /// (see [`SpringTrack`]).
    pub fn integrate(&self, mut x: f64, mut v: f64, target: f64, dt_ms: f64, precision: f64) -> (f64, f64) {
        if dt_ms <= 0.0 {
            return (x, v);
        }
        if self.is_instant() {
            return (target, 0.0);
        }
        let k = self.stiffness;
        let m = self.mass;
        let mut c = self.damping;
        if self.clamp {
            c = clamp_damping(k, m, c, x, v, target);
        }
        let n = dt_ms.ceil().max(1.0) as usize;
        if n > 10_000 {
            return (target, 0.0);
        }
        for i in 1..=n {
            let h = if i as f64 > dt_ms { (i as f64 - dt_ms) / 1000.0 } else { 0.001 };
            let a = (-(x - target) * k - v * c) / m;
            let v2 = v + a * h;
            let x2 = x + v2 * h;
            if v2.abs() < precision && (x2 - target).abs() < precision {
                return (target, 0.0);
            }
            x = x2;
            v = v2;
        }
        (x, v)
    }
}

/// Screen Studio's `clamp`: when moving toward the target faster than a critically damped
/// spring would, use critical damping so the motion cannot overshoot.
fn clamp_damping(k: f64, m: f64, c: f64, x: f64, v: f64, target: f64) -> f64 {
    let d = target - x;
    if !(v * d > 0.0) {
        return c;
    }
    let w = (k / m).sqrt();
    let vmax = d.abs() * w;
    if v.abs() <= vmax {
        return c;
    }
    c.max(2.0 * (m * k).sqrt())
}

impl From<Spring> for SpringParams {
    fn from(s: Spring) -> Self {
        SpringParams { stiffness: s.stiffness, damping: s.damping, mass: s.mass, clamp: s.clamp.unwrap_or(true), precision: s.precision.unwrap_or(0.002) }
    }
}

impl From<Option<Spring>> for SpringParams {
    fn from(s: Option<Spring>) -> Self {
        s.map(Into::into).unwrap_or(SpringParams::INSTANT)
    }
}

/// One animated scalar with target tracking and distance-scaled precision.
#[derive(Debug, Clone, Copy)]
pub struct SpringTrack {
    pub params: SpringParams,
    pub state: SpringState,
    target: f64,
    precision: f64,
}

impl SpringTrack {
    pub fn new(params: SpringParams, value: f64) -> Self {
        SpringTrack { params, state: SpringState::at(value), target: value, precision: params.precision }
    }

    pub fn snap(&mut self, value: f64) {
        self.state = SpringState::at(value);
        self.target = value;
        self.precision = self.params.precision;
    }

    pub fn set_target(&mut self, target: f64) {
        if (target - self.target).abs() > f64::EPSILON {
            self.precision = (self.state.position - target).abs().max(1.0) * self.params.precision;
            self.target = target;
        }
    }

    pub fn step(&mut self, target: f64, dt_ms: f64) -> f64 {
        self.set_target(target);
        let (x, v) = self.params.integrate(self.state.position, self.state.velocity, self.target, dt_ms, self.precision);
        self.state = SpringState { position: x, velocity: v };
        x
    }

    pub fn value(&self) -> f64 {
        self.state.position
    }

    pub fn is_settled(&self) -> bool {
        self.state.velocity.abs() < self.precision && (self.state.position - self.target).abs() < self.precision
    }
}

/// N scalars animated with the same config (Screen Studio's `ObjectSpring` over a rect/point).
#[derive(Debug, Clone)]
pub struct SpringVec<const N: usize> {
    pub tracks: [SpringTrack; N],
}

impl<const N: usize> SpringVec<N> {
    pub fn new(params: SpringParams, initial: [f64; N]) -> Self {
        SpringVec { tracks: initial.map(|v| SpringTrack::new(params, v)) }
    }

    pub fn set_params(&mut self, params: SpringParams) {
        for t in &mut self.tracks {
            t.params = params;
        }
    }

    pub fn snap(&mut self, to: [f64; N]) {
        for (t, v) in self.tracks.iter_mut().zip(to) {
            t.snap(v);
        }
    }

    pub fn step(&mut self, target: [f64; N], dt_ms: f64) -> [f64; N] {
        let mut out = [0.0; N];
        for i in 0..N {
            out[i] = self.tracks[i].step(target[i], dt_ms);
        }
        out
    }

    pub fn position(&self) -> [f64; N] {
        self.tracks.map(|t| t.value())
    }

    pub fn velocity(&self) -> [f64; N] {
        self.tracks.map(|t| t.state.velocity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converges() {
        let mut s = SpringTrack::new(SpringParams::DEFAULT, 0.0);
        let mut t = 0.0;
        loop {
            s.step(1.0, 16.0);
            t += 16.0;
            if s.is_settled() || t > 10_000.0 {
                break;
            }
        }
        assert!(t < 5000.0, "spring should settle in a few seconds, took {t}ms");
        assert_eq!(s.value(), 1.0);
    }

    #[test]
    fn clamp_prevents_overshoot() {
        let p = SpringParams { stiffness: 470.0, damping: 5.0, mass: 1.0, clamp: true, precision: 0.001 };
        let mut s = SpringTrack::new(p, 0.0);
        for _ in 0..500 {
            let x = s.step(100.0, 8.0);
            // Euler sub-steps may overshoot by a hair; Screen Studio behaves the same.
            assert!(x <= 100.0 + 0.05, "overshot to {x}");
        }
    }

    #[test]
    fn instant_snaps() {
        let mut s = SpringTrack::new(SpringParams::INSTANT, 0.0);
        assert_eq!(s.step(5.0, 1.0), 5.0);
    }
}
