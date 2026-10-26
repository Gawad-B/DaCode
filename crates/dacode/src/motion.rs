//! Springs, with time as a parameter.
//!
//! Every animated quantity in the editor (a panel sliding in, a popup arriving, a scroll catching up)
//! is a [`Value`] chasing a target under a damped spring. The design rule that everything else
//! follows from is that **`get` is a pure function of the instant it is asked about**: it evaluates
//! the spring's closed form from the moment the value was last retargeted, rather than being
//! stepped once per frame. That is what lets a test render any instant it likes, lets a frame that
//! was dropped catch up exactly instead of integrating a long step badly, and means nothing has to
//! be ticked for the answer to be right. The only thing the event loop needs from here is
//! [`Motion::busy`], so that an editor with nothing moving can go back to sleep.
//!
//! The spring is `x'' = -k (x - target) - c x'` with unit mass, so `omega = sqrt(k)` and
//! `c = 2 zeta omega`. Written in terms of the displacement `d = x - target`, with `d0` and `v0` the
//! displacement and velocity at the moment of retargeting, there are three closed forms, and which
//! one applies is decided by the damping ratio `zeta` alone.

use std::collections::HashMap;
use std::time::Instant;

/// How much the interface moves. A user setting; `Off` exists for people who find motion
/// uncomfortable as much as for people on a slow machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Animations {
    /// Springs as designed, including the little overshoot on panels and popups.
    #[default]
    Full,
    /// The same travel without any overshoot: nothing ever goes past where it is going.
    Reduced,
    /// Everything jumps.
    Off,
}

impl Animations {
    pub const ALL: [Animations; 3] = [Animations::Full, Animations::Reduced, Animations::Off];

    /// The spelling used in `init.lua` and in the settings panel.
    pub fn name(self) -> &'static str {
        match self {
            Animations::Full => "full",
            Animations::Reduced => "reduced",
            Animations::Off => "off",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.name() == name)
    }
}

/// A spring's character: how stiff it is, and how much it rings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spring {
    /// `k` in `x'' = -k (x - target) - c x'`. Higher arrives sooner.
    pub stiffness: f32,
    /// `zeta`. Below 1 the value overshoots and settles; at 1 it arrives as fast as it can without
    /// overshooting; above 1 it creeps in.
    pub damping_ratio: f32,
}

impl Spring {
    /// Sidebar, palette and other large surfaces: a visible, brief overshoot (about 4%).
    pub const PANEL: Spring = Spring {
        stiffness: 180.0,
        damping_ratio: 0.72,
    };
    /// Menus and popups: quicker, and a touch firmer than a panel.
    pub const POPUP: Spring = Spring {
        stiffness: 260.0,
        damping_ratio: 0.78,
    };
    /// Hover feedback: fast and never overshooting, since it fires constantly.
    pub const HOVER: Spring = Spring {
        stiffness: 400.0,
        damping_ratio: 1.0,
    };
    /// Scrolling. Critically damped on purpose: text that bounces past where it was scrolled to
    /// reads as the page being loose, not as polish.
    pub const SCROLL: Spring = Spring {
        stiffness: 320.0,
        damping_ratio: 1.0,
    };
}

/// What is being animated. A key per thing, so two retargets of the same thing share momentum
/// instead of two animations fighting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    Sidebar,
    Palette,
    Menu,
    Completion,
    Info,
    Help,
    Notifications,
    Hover(u32),
    TabIndicator,
    ActivityIndicator,
    Scroll(usize),
    ScrollX(usize),
    Bell,
    Welcome,
    SettingsCategory,
    Glow(u32),
}

/// One animated quantity. `position` and `velocity` are what they were at `since`; the present is
/// derived from them, never stored.
#[derive(Debug, Clone, Copy)]
struct Value {
    position: f64,
    velocity: f64,
    target: f64,
    /// Already clamped for `Animations::Reduced` when it was stored.
    spring: Spring,
    since: Instant,
}

impl Value {
    /// Position and velocity at `now`. Asking about an instant before `since` answers as of `since`.
    fn sample(&self, now: Instant) -> (f64, f64) {
        let t = now.saturating_duration_since(self.since).as_secs_f64();
        let (d, v) = evaluate(self.position - self.target, self.velocity, self.spring, t);
        (self.target + d, v)
    }

    fn is_settled(&self, now: Instant) -> bool {
        let (position, velocity) = self.sample(now);
        settled(position, velocity, self.target)
    }
}

/// Close enough to stop asking for frames: well under a pixel of a pixel-scale value, and under
/// a thousandth of a unit-range one.
fn settled(position: f64, velocity: f64, target: f64) -> bool {
    let scale = target.abs().max(1.0);
    (position - target).abs() < 1e-3 * scale && velocity.abs() < 1e-2 * scale
}

/// Within this of exactly 1 the damping is treated as critical. The under- and over-damped forms
/// both divide by something that goes to zero there, and the critical form is the limit of both.
const CRITICAL_BAND: f64 = 1e-4;

/// The damped spring's displacement and velocity `t` seconds after it was at `(d0, v0)`.
///
/// With `omega = sqrt(k)` and `zeta` the damping ratio:
///
/// * Under-damped (`zeta < 1`), `wd = omega sqrt(1 - zeta^2)`, `a = zeta omega`:
///   `d = e^{-a t} (d0 cos wd t + (v0 + a d0) / wd sin wd t)`.
/// * Critically damped (`zeta = 1`):
///   `d = e^{-omega t} (d0 + (v0 + omega d0) t)`.
/// * Over-damped (`zeta > 1`), `r = omega sqrt(zeta^2 - 1)`, roots `s1,2 = -zeta omega +- r`:
///   `d = c1 e^{s1 t} + c2 e^{s2 t}` with `c2 = (s1 d0 - v0) / (s1 - s2)` and `c1 = d0 - c2`.
///
/// Velocity is the derivative of each, written out rather than differenced, so a retarget carries
/// the exact momentum.
fn evaluate(d0: f64, v0: f64, spring: Spring, t: f64) -> (f64, f64) {
    let omega = f64::from(spring.stiffness).max(f64::EPSILON).sqrt();
    let zeta = f64::from(spring.damping_ratio).max(0.0);

    if (zeta - 1.0).abs() < CRITICAL_BAND {
        let decay = (-omega * t).exp();
        let b = v0 + omega * d0;
        (decay * (d0 + b * t), decay * (v0 - omega * b * t))
    } else if zeta < 1.0 {
        let a = zeta * omega;
        let wd = omega * (1.0 - zeta * zeta).sqrt();
        let decay = (-a * t).exp();
        let (sin, cos) = (wd * t).sin_cos();
        let b = (v0 + a * d0) / wd;
        (
            decay * (d0 * cos + b * sin),
            decay * (v0 * cos - (a * v0 + omega * omega * d0) / wd * sin),
        )
    } else {
        let r = omega * (zeta * zeta - 1.0).sqrt();
        let (s1, s2) = (-zeta * omega + r, -zeta * omega - r);
        let c2 = (s1 * d0 - v0) / (s1 - s2);
        let c1 = d0 - c2;
        let (e1, e2) = ((s1 * t).exp(), (s2 * t).exp());
        (c1 * e1 + c2 * e2, c1 * s1 * e1 + c2 * s2 * e2)
    }
}

/// Every animated quantity in the editor, and what the user said about animating them.
///
/// The executor and the drawing code never hold a time of their own: they are handed `now`, so a
/// test can ask what the screen looked like 60 ms into a transition without waiting 60 ms.
#[derive(Debug, Clone, Default)]
pub struct Motion {
    values: HashMap<Key, Value>,
    animations: Animations,
}

impl Motion {
    pub fn new(animations: Animations) -> Self {
        Self {
            values: HashMap::new(),
            animations,
        }
    }

    /// Switching to `Off` finishes whatever is in flight on the spot: otherwise a user who has just
    /// asked for no motion would watch the last transition play out, and `busy` would keep waking
    /// the loop for it.
    pub fn set_animations(&mut self, animations: Animations) {
        self.animations = animations;
        if animations == Animations::Off {
            let keys: Vec<Key> = self.values.keys().copied().collect();
            for key in keys {
                if let Some(target) = self.target(key) {
                    self.snap(key, target);
                }
            }
        }
    }

    /// Send `key` towards `target`, keeping whatever velocity it has at `now`. Momentum is the point
    /// of using a spring: a sidebar toggled twice in quick succession reverses smoothly instead of
    /// restarting from where a timeline says it should be.
    pub fn to(&mut self, key: Key, target: f32, spring: Spring, now: Instant) {
        if self.animations == Animations::Off {
            self.snap(key, target);
            return;
        }
        // A NaN would never compare as settled, and `busy` would then keep the loop awake forever.
        if !target.is_finite() {
            return;
        }
        let spring = if self.animations == Animations::Reduced {
            Spring {
                damping_ratio: spring.damping_ratio.max(1.0),
                ..spring
            }
        } else {
            spring
        };
        let (position, velocity) = self
            .values
            .get(&key)
            .map_or((0.0, 0.0), |value| value.sample(now));
        self.values.insert(
            key,
            Value {
                position,
                velocity,
                target: f64::from(target),
                spring,
                since: now,
            },
        );
    }

    /// Jump to `value` with no velocity.
    pub fn snap(&mut self, key: Key, value: f32) {
        if !value.is_finite() {
            return;
        }
        // A value at rest on its target evaluates to its target at every instant, so `since` is
        // irrelevant and the caller does not need to supply one.
        self.values.insert(
            key,
            Value {
                position: f64::from(value),
                velocity: 0.0,
                target: f64::from(value),
                spring: Spring::HOVER,
                since: Instant::now(),
            },
        );
    }

    /// Where `key` is at `now`; `0.0` for a key that was never set. Once it is within the settling
    /// tolerance this returns the target exactly, so a caller comparing against it sees arrival.
    pub fn get(&self, key: Key, now: Instant) -> f32 {
        let Some(value) = self.values.get(&key) else {
            return 0.0;
        };
        let (position, velocity) = value.sample(now);
        if settled(position, velocity, value.target) {
            value.target as f32
        } else {
            position as f32
        }
    }

    /// Where `key` is heading, if it has ever been set.
    pub fn target(&self, key: Key) -> Option<f32> {
        self.values.get(&key).map(|value| value.target as f32)
    }

    /// Whether anything is still moving at `now`, which is the only thing the event loop asks.
    pub fn busy(&self, now: Instant) -> bool {
        self.values.values().any(|value| !value.is_settled(now))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::time::Duration;

    fn at(start: Instant, ms: u64) -> Instant {
        start + Duration::from_millis(ms)
    }

    fn peak(motion: &Motion, key: Key, start: Instant) -> f32 {
        (0..400)
            .map(|step| motion.get(key, at(start, step * 5)))
            .fold(f32::MIN, f32::max)
    }

    #[test]
    fn a_spring_arrives_and_then_the_editor_can_sleep() {
        let start = Instant::now();
        let mut motion = Motion::new(Animations::Full);
        motion.to(Key::Sidebar, 1.0, Spring::PANEL, start);
        assert!(motion.busy(at(start, 16)));
        assert!((motion.get(Key::Sidebar, at(start, 2000)) - 1.0).abs() < 1e-3);
        assert!(
            !motion.busy(at(start, 2000)),
            "settled means no more frames are asked for"
        );
    }

    #[test]
    fn the_expressive_spring_overshoots_a_little_and_the_scroll_spring_never_does() {
        let start = Instant::now();
        let mut motion = Motion::new(Animations::Full);
        motion.to(Key::Palette, 1.0, Spring::PANEL, start);
        motion.to(Key::Scroll(0), 100.0, Spring::SCROLL, start);
        let overshoot = peak(&motion, Key::Palette, start);
        assert!(
            overshoot > 1.01 && overshoot < 1.15,
            "overshoot {overshoot}"
        );
        assert!(
            peak(&motion, Key::Scroll(0), start) <= 100.0 + 1e-3,
            "text must not bounce past where it was scrolled to"
        );
    }

    #[test]
    fn retargeting_mid_flight_keeps_momentum() {
        let start = Instant::now();
        let mut motion = Motion::new(Animations::Full);
        motion.to(Key::Sidebar, 1.0, Spring::PANEL, start);
        let mid = at(start, 60);
        let before = motion.get(Key::Sidebar, mid);
        motion.to(Key::Sidebar, 0.0, Spring::PANEL, mid);
        assert!(
            motion.get(Key::Sidebar, at(start, 70)) > before,
            "still travelling outward a moment after being sent back"
        );
    }

    #[test]
    fn off_jumps_and_reduced_never_overshoots() {
        let start = Instant::now();
        let mut off = Motion::new(Animations::Off);
        off.to(Key::Sidebar, 1.0, Spring::PANEL, start);
        assert_eq!(off.get(Key::Sidebar, start), 1.0);
        assert!(!off.busy(start));

        let mut reduced = Motion::new(Animations::Reduced);
        reduced.to(Key::Palette, 1.0, Spring::PANEL, start);
        assert!(peak(&reduced, Key::Palette, start) <= 1.0 + 1e-3);
    }

    #[test]
    fn retargeting_does_not_move_the_value_at_the_instant_it_happens() {
        // A jump here would show as a one-frame flash every time a toggle was pressed mid-animation.
        let start = Instant::now();
        let mut motion = Motion::new(Animations::Full);
        motion.to(Key::Sidebar, 1.0, Spring::PANEL, start);
        let mid = at(start, 90);
        let before = motion.get(Key::Sidebar, mid);
        motion.to(Key::Sidebar, -3.0, Spring::POPUP, mid);
        assert!((motion.get(Key::Sidebar, mid) - before).abs() < 1e-6);
    }

    #[test]
    fn a_key_never_set_reads_zero_and_has_no_target() {
        let motion = Motion::new(Animations::Full);
        assert_eq!(motion.get(Key::Hover(7), Instant::now()), 0.0);
        assert_eq!(motion.target(Key::Hover(7)), None);
        assert!(!motion.busy(Instant::now()));
    }

    #[test]
    fn snapping_sets_the_value_and_the_target_with_nothing_left_to_animate() {
        let start = Instant::now();
        let mut motion = Motion::new(Animations::Full);
        motion.to(Key::Bell, 5.0, Spring::PANEL, start);
        motion.snap(Key::Bell, 2.0);
        assert_eq!(motion.get(Key::Bell, start), 2.0);
        assert_eq!(motion.get(Key::Bell, at(start, 5000)), 2.0);
        assert_eq!(motion.target(Key::Bell), Some(2.0));
        assert!(!motion.busy(start));
    }

    #[test]
    fn turning_animations_off_finishes_what_is_in_flight() {
        // Otherwise `busy` keeps waking the loop for a transition the user just asked not to see.
        let start = Instant::now();
        let mut motion = Motion::new(Animations::Full);
        motion.to(Key::Sidebar, 1.0, Spring::PANEL, start);
        motion.set_animations(Animations::Off);
        assert_eq!(motion.get(Key::Sidebar, at(start, 10)), 1.0);
        assert!(!motion.busy(at(start, 10)));
    }

    #[test]
    fn a_non_finite_target_is_refused_rather_than_keeping_the_loop_awake() {
        let start = Instant::now();
        let mut motion = Motion::new(Animations::Full);
        motion.to(Key::Sidebar, f32::NAN, Spring::PANEL, start);
        motion.to(Key::Menu, f32::INFINITY, Spring::PANEL, start);
        assert_eq!(motion.target(Key::Sidebar), None);
        assert!(!motion.busy(at(start, 1000)));
    }

    #[test]
    fn asking_about_a_moment_before_the_animation_began_answers_as_of_its_start() {
        let start = Instant::now();
        let mut motion = Motion::new(Animations::Full);
        motion.to(Key::Sidebar, 1.0, Spring::PANEL, at(start, 100));
        assert_eq!(motion.get(Key::Sidebar, start), 0.0);
    }

    #[test]
    fn animation_names_round_trip_and_reject_anything_else() {
        for animations in Animations::ALL {
            assert_eq!(Animations::from_name(animations.name()), Some(animations));
        }
        assert_eq!(Animations::from_name("fast"), None);
        assert_eq!(Animations::default(), Animations::Full);
    }

    /// An independent check on the closed forms: step the differential equation itself with RK4 and
    /// compare. Each regime has a different formula, so each gets its own start state, including a
    /// nonzero velocity, because that is the case a retarget produces.
    #[test]
    fn every_damping_regime_agrees_with_integrating_the_equation_directly() {
        fn rk4(d0: f64, v0: f64, spring: Spring, t: f64) -> (f64, f64) {
            let k = f64::from(spring.stiffness);
            let c = 2.0 * f64::from(spring.damping_ratio) * k.sqrt();
            let accel = |d: f64, v: f64| -k * d - c * v;
            let steps = 20_000;
            let h = t / f64::from(steps);
            let (mut d, mut v) = (d0, v0);
            for _ in 0..steps {
                let (k1d, k1v) = (v, accel(d, v));
                let (k2d, k2v) = (
                    v + 0.5 * h * k1v,
                    accel(d + 0.5 * h * k1d, v + 0.5 * h * k1v),
                );
                let (k3d, k3v) = (
                    v + 0.5 * h * k2v,
                    accel(d + 0.5 * h * k2d, v + 0.5 * h * k2v),
                );
                let (k4d, k4v) = (v + h * k3v, accel(d + h * k3d, v + h * k3v));
                d += h / 6.0 * (k1d + 2.0 * k2d + 2.0 * k3d + k4d);
                v += h / 6.0 * (k1v + 2.0 * k2v + 2.0 * k3v + k4v);
            }
            (d, v)
        }

        for ratio in [0.3, 0.72, 0.99, 1.0, 1.01, 2.5] {
            let spring = Spring {
                stiffness: 250.0,
                damping_ratio: ratio,
            };
            for t in [0.01, 0.1, 0.4, 1.0] {
                let (d, v) = evaluate(-1.0, 6.0, spring, t);
                let (rd, rv) = rk4(-1.0, 6.0, spring, t);
                assert!((d - rd).abs() < 1e-6, "zeta {ratio} t {t}: d {d} vs {rd}");
                assert!((v - rv).abs() < 1e-5, "zeta {ratio} t {t}: v {v} vs {rv}");
            }
        }
    }

    const SPRINGS: [Spring; 4] = [Spring::PANEL, Spring::POPUP, Spring::HOVER, Spring::SCROLL];

    proptest! {
        /// Whatever the user does to a toggle, however fast, the value stays a number and comes to
        /// rest: the property the event loop's ability to sleep rests on.
        #[test]
        fn any_run_of_retargets_stays_finite_and_settles_within_three_seconds(
            steps in proptest::collection::vec(
                (-1.0e4f32..1.0e4, 1u64..500, 0usize..SPRINGS.len()),
                1..=8,
            ),
            animations in 0usize..2,
        ) {
            let start = Instant::now();
            let mut motion = Motion::new(Animations::ALL[animations]);
            let mut clock = 0;
            for (target, gap, spring) in steps {
                clock += gap;
                motion.to(Key::Sidebar, target, SPRINGS[spring], at(start, clock));
                for sample in 0..=10 {
                    let value = motion.get(Key::Sidebar, at(start, clock + gap * sample / 10));
                    prop_assert!(value.is_finite());
                }
            }
            let later = at(start, clock + 3000);
            prop_assert!(motion.get(Key::Sidebar, later).is_finite());
            prop_assert!(!motion.busy(later), "still moving three seconds after the last retarget");
            prop_assert_eq!(
                motion.get(Key::Sidebar, later),
                motion.target(Key::Sidebar).unwrap()
            );
        }
    }
}
