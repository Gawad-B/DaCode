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

    /// The spelling used in `init.lua` and on the settings tab.
    pub const fn name(self) -> &'static str {
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
    /// The bell's wobble: loose and quick, so a kick rings a few times and is gone in under a second.
    /// Only ever kicked, never retargeted — a wobble is motion about where the value rests.
    pub const WOBBLE: Spring = Spring {
        stiffness: 400.0,
        damping_ratio: 0.2,
    };
    /// The push that swings a [`Spring::WOBBLE`] about one unit out before it rings down.
    pub const WOBBLE_KICK: f32 = 26.0;
    /// A settings row that has just changed, fading back from lit: slow enough to be seen from the
    /// other side of the page, and without overshoot, since a glow that went below nothing would
    /// flash back on.
    pub const GLOW: Spring = Spring {
        stiffness: 30.0,
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
///
/// Entries are never removed and `busy` visits every one, so an index in a variant must come from a
/// bounded set (a pane number, an activity icon), not from something that grows with the project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    Sidebar,
    Palette,
    Menu,
    Completion,
    Info,
    Help,
    Notifications,
    /// An activity-bar icon under the pointer, by its index in `layout::ACTIVITIES`. Anything else
    /// that lifts on hover needs a variant of its own, or it shares springs with these.
    Hover(u32),
    TabIndicator,
    ActivityIndicator,
    Scroll(usize),
    ScrollX(usize),
    /// The settings tab's list, which is not a pane and must not share a code pane's spring.
    SettingsScroll,
    Bell,
    Welcome,
    SettingsCategory,
    Glow(u32),
    /// A tab that has just been opened, growing in.
    TabGrow,
    /// The settings tab's theme dropdown.
    Dropdown,
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
    /// When `position` and `velocity` were taken. `None` for a value at rest after a `snap`: it has
    /// no timeline to anchor, and giving it a clock reading would make a later `to` from an earlier
    /// instant (a capture at a chosen time) start late.
    since: Option<Instant>,
    /// How close counts as arrived. Fixed at the retarget, from how far the value has to go, and not
    /// from where the target happens to sit: a scroll offset of a hundred thousand pixels moved by a
    /// wheel notch must still animate, and a large move must not keep asking for frames while it
    /// creeps by a fraction of a pixel.
    position_tolerance: f64,
    velocity_tolerance: f64,
}

impl Value {
    /// Position and velocity at `now`. Asking about an instant before `since` answers as of `since`.
    fn sample(&self, now: Instant) -> (f64, f64) {
        let Some(since) = self.since else {
            return (self.position, self.velocity);
        };
        let t = now.saturating_duration_since(since).as_secs_f64();
        let (d, v) = evaluate(self.position - self.target, self.velocity, self.spring, t);
        (self.target + d, v)
    }

    /// A value at rest on `target`, which is settled at every instant.
    fn at_rest(target: f64) -> Self {
        let (position_tolerance, velocity_tolerance) = tolerances(0.0);
        Self {
            position: target,
            velocity: 0.0,
            target,
            spring: Spring::HOVER,
            since: None,
            position_tolerance,
            velocity_tolerance,
        }
    }

    fn is_settled(&self, position: f64, velocity: f64) -> bool {
        (position - self.target).abs() < self.position_tolerance
            && velocity.abs() < self.velocity_tolerance
    }
}

/// Close enough to stop asking for frames, for a move of `travel`: a thousandth of it, but never more
/// than half a pixel of a pixel-valued key, and never so little that rounding keeps it awake.
/// Velocity is looser by a decade, as a value arriving slowly is as good as arrived.
fn tolerances(travel: f64) -> (f64, f64) {
    (
        (1e-3 * travel).clamp(1e-4, 0.5),
        (1e-2 * travel).clamp(1e-3, 5.0),
    )
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

    /// What the user asked for, so the settings tab can show the live value rather than a copy.
    pub fn animations(&self) -> Animations {
        self.animations
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

    /// Send `key` towards `target`, keeping whatever velocity it has at `now`. Asking for the target
    /// it already has changes nothing. Momentum is the point
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
        // `Spring`'s fields are public. A zero or NaN would give an undamped or glacial spring that
        // never settles, and `busy` would keep the loop awake for good.
        let spring = Spring {
            stiffness: spring.stiffness.max(1.0),
            damping_ratio: spring.damping_ratio.max(0.05),
        };
        let target = f64::from(target);
        // Drawing code can say where something should be every frame. Asking for where it is already
        // heading must leave the trajectory alone, not restart it from the current instant (which would
        // also forget the tolerance it was given at the start of the move). Exact equality is the
        // test: a target that differs at all is a retarget. The spring passed is ignored here too.
        if self
            .values
            .get(&key)
            .is_some_and(|value| value.target == target)
        {
            return;
        }
        let ((position, mut velocity), since) = match self.values.get(&key) {
            // An earlier `now` than the value's own clock must not rewind it: the timeline would then
            // replay motion that never happened.
            Some(value) => (value.sample(now), value.since.map_or(now, |s| now.max(s))),
            None => ((0.0, 0.0), now),
        };
        let omega = f64::from(spring.stiffness).sqrt();
        let zeta = f64::from(spring.damping_ratio);
        if zeta >= 1.0 {
            // Without overshoot from rest is not without overshoot from motion: retargeting to a nearer
            // point while still travelling fast carries the value through it. This is the largest
            // velocity toward the target that still arrives without crossing it (omega * |d0| when
            // critically damped), so momentum is kept up to the point where it would cost the promise.
            let d0 = position - target;
            let bound = omega * (zeta + (zeta * zeta - 1.0).sqrt()) * d0.abs();
            if velocity * d0 < 0.0 && velocity.abs() > bound {
                velocity = -d0.signum() * bound;
            }
        }
        let (position_tolerance, velocity_tolerance) =
            tolerances((target - position).abs().max(velocity.abs() / omega));
        self.values.insert(
            key,
            Value {
                position,
                velocity,
                target,
                spring,
                since: Some(since),
                position_tolerance,
                velocity_tolerance,
            },
        );
    }

    /// Jump to `value` with no velocity.
    pub fn snap(&mut self, key: Key, value: f32) {
        if !value.is_finite() {
            return;
        }
        // At rest there is no timeline, so the caller does not need to supply a clock.
        self.values.insert(key, Value::at_rest(f64::from(value)));
    }

    /// Where `key` is at `now`; `0.0` for a key that was never set. Once it is within the settling
    /// tolerance this returns the target exactly, so a caller comparing against it sees arrival.
    pub fn get(&self, key: Key, now: Instant) -> f32 {
        let Some(value) = self.values.get(&key) else {
            return 0.0;
        };
        let (position, velocity) = value.sample(now);
        if value.is_settled(position, velocity) {
            value.target as f32
        } else {
            position as f32
        }
    }

    /// Where `key` is at `now` if it is heading for `expected`, and `expected` if it is not.
    ///
    /// For drawing, which knows from the state where a value ought to be going. A value that was
    /// never set, or whose target is stale because nothing has retargeted it since the state changed,
    /// must not draw an open view as closed: the state wins, and the view is drawn where it belongs.
    pub fn fade(&self, key: Key, expected: f32, now: Instant) -> f32 {
        match self.values.get(&key) {
            Some(value) if value.target == f64::from(expected) => self.get(key, now),
            _ => expected,
        }
    }

    /// [`Motion::fade`] for a value that moves something on screen — a width, a slide, a scale, a
    /// lift. Under [`Animations::Reduced`] it is already where it is going: that setting promises
    /// fades only, and a sidebar that slid, however gently, would break it.
    pub fn movement(&self, key: Key, expected: f32, now: Instant) -> f32 {
        if self.animations == Animations::Reduced {
            expected
        } else {
            self.fade(key, expected, now)
        }
    }

    /// Pushes `key` with `velocity` units per second at `now`, leaving its target where it is: a
    /// wobble about where the value rests, rather than a move to somewhere else.
    ///
    /// Does nothing unless motion is [`Animations::Full`]. A kick is all movement, which Reduced
    /// promises not to show, and asking for frames to draw nothing would only keep the loop awake.
    pub fn kick(&mut self, key: Key, velocity: f32, spring: Spring, now: Instant) {
        if self.animations != Animations::Full || !velocity.is_finite() {
            return;
        }
        let spring = Spring {
            stiffness: spring.stiffness.max(1.0),
            damping_ratio: spring.damping_ratio.max(0.05),
        };
        let (position, moving, target, since) = match self.values.get(&key) {
            Some(value) => {
                let (position, moving) = value.sample(now);
                (
                    position,
                    moving,
                    value.target,
                    value.since.map_or(now, |s| now.max(s)),
                )
            }
            None => (0.0, 0.0, 0.0, now),
        };
        // Kicks in quick succession do not add up: a second message rings the bell again, not
        // twice as hard into the clamp drawing puts on it.
        let kick = f64::from(velocity);
        let velocity = (moving + kick).clamp(-kick.abs(), kick.abs());
        let omega = f64::from(spring.stiffness).sqrt();
        let (position_tolerance, velocity_tolerance) =
            tolerances((target - position).abs().max(velocity.abs() / omega));
        self.values.insert(
            key,
            Value {
                position,
                velocity,
                target,
                spring,
                since: Some(since),
                position_tolerance,
                velocity_tolerance,
            },
        );
    }

    /// Where `key` is heading, if it has ever been set.
    pub fn target(&self, key: Key) -> Option<f32> {
        self.values.get(&key).map(|value| value.target as f32)
    }

    /// Whether anything is still moving at `now`, which is the only thing the event loop asks.
    pub fn busy(&self, now: Instant) -> bool {
        self.values.values().any(|value| {
            let (position, velocity) = value.sample(now);
            !value.is_settled(position, velocity)
        })
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
    fn a_value_whose_target_is_stale_reads_as_the_state_it_should_follow() {
        // Drawing asks "where is the palette, given that it is open?". A motion store nobody has
        // synced since the palette opened — a test that sets a field and draws, the first frame
        // after start-up — must answer "open", or the view would be drawn as if it were closed.
        let start = Instant::now();
        let mut motion = Motion::new(Animations::Full);
        assert_eq!(motion.fade(Key::Palette, 1.0, start), 1.0, "never set");
        motion.to(Key::Palette, 0.0, Spring::POPUP, start);
        assert_eq!(
            motion.fade(Key::Palette, 1.0, start),
            1.0,
            "heading elsewhere"
        );
        motion.to(Key::Palette, 1.0, Spring::POPUP, start);
        let mid = motion.fade(Key::Palette, 1.0, at(start, 60));
        assert!(mid > 0.0 && mid < 1.0, "in flight: {mid}");
    }

    #[test]
    fn reduced_motion_fades_but_never_moves() {
        // The setting promises fades only. A value read as a fade still travels; one read as
        // movement is already where it is going.
        let start = Instant::now();
        let mut motion = Motion::new(Animations::Reduced);
        motion.to(Key::Sidebar, 1.0, Spring::PANEL, start);
        let mid = at(start, 80);
        assert_eq!(motion.movement(Key::Sidebar, 1.0, mid), 1.0);
        let fade = motion.fade(Key::Sidebar, 1.0, mid);
        assert!(fade > 0.0 && fade < 1.0, "{fade}");

        let mut full = Motion::new(Animations::Full);
        full.to(Key::Sidebar, 1.0, Spring::PANEL, start);
        assert!(full.movement(Key::Sidebar, 1.0, mid) < 1.0);
    }

    #[test]
    fn a_kick_wobbles_about_where_a_value_rests_and_dies_away() {
        let start = Instant::now();
        let mut motion = Motion::new(Animations::Full);
        motion.kick(Key::Bell, Spring::WOBBLE_KICK, Spring::WOBBLE, start);
        assert!(motion.busy(at(start, 16)));
        let samples: Vec<f32> = (0..200)
            .map(|ms| motion.get(Key::Bell, at(start, ms * 5)))
            .collect();
        let high = samples.iter().copied().fold(f32::MIN, f32::max);
        let low = samples.iter().copied().fold(f32::MAX, f32::min);
        assert!(
            high > 0.8 && high < 1.2,
            "swings out about one unit: {high}"
        );
        assert!(low < -0.2, "and back past where it rests: {low}");
        assert_eq!(motion.target(Key::Bell), Some(0.0));
        assert!(!motion.busy(at(start, 3000)), "and then it stops");
    }

    #[test]
    fn a_kick_moves_nothing_when_motion_is_reduced_or_off() {
        // A wobble is all movement and no fade, so there is nothing of it left to show.
        let start = Instant::now();
        for animations in [Animations::Reduced, Animations::Off] {
            let mut motion = Motion::new(animations);
            motion.kick(Key::Bell, Spring::WOBBLE_KICK, Spring::WOBBLE, start);
            assert_eq!(motion.get(Key::Bell, at(start, 40)), 0.0);
            assert!(!motion.busy(at(start, 16)));
        }
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

    #[test]
    fn a_wheel_notch_deep_in_a_large_file_still_animates_and_settles() {
        // A scroll key is a cumulative pixel offset. With a tolerance proportional to the target, a
        // 60 px notch at 100,000 px counted as arrived before it started and scrolling never moved.
        let start = Instant::now();
        let mut motion = Motion::new(Animations::Full);
        motion.snap(Key::Scroll(0), 100_000.0);
        motion.to(Key::Scroll(0), 100_060.0, Spring::SCROLL, start);
        assert!(motion.busy(at(start, 16)));
        let mid = motion.get(Key::Scroll(0), at(start, 16));
        assert!(mid > 100_000.0 && mid < 100_060.0, "mid-flight {mid}");
        assert_eq!(motion.get(Key::Scroll(0), at(start, 3000)), 100_060.0);
        assert!(!motion.busy(at(start, 3000)));
    }

    #[test]
    fn a_unit_range_value_settles_to_the_same_thousandth_as_before() {
        let start = Instant::now();
        let mut motion = Motion::new(Animations::Full);
        motion.to(Key::Sidebar, 1.0, Spring::PANEL, start);
        assert!(motion.busy(at(start, 300)));
        assert!(!motion.busy(at(start, 2000)));
    }

    #[test]
    fn a_large_move_stops_asking_for_frames_once_it_is_within_half_a_pixel() {
        // Relative to the target, 10,000 px down to 0 kept the loop awake for 1.65 s of sub-pixel motion.
        let start = Instant::now();
        let mut motion = Motion::new(Animations::Full);
        motion.snap(Key::Scroll(0), 10_000.0);
        motion.to(Key::Scroll(0), 0.0, Spring::PANEL, start);
        let arrived = (0..3000)
            .find(|&ms| !motion.busy(at(start, ms)))
            .expect("settles");
        assert!(arrived < 1300, "still busy at {arrived} ms");
        let last_frame = motion.get(Key::Scroll(0), at(start, arrived - 1));
        assert!(
            last_frame.abs() < 1.0,
            "the last drawn frame was {last_frame} px away"
        );
    }

    #[test]
    fn retargeting_nearer_mid_flight_does_not_carry_a_critical_spring_past_it() {
        // From rest these springs cannot overshoot, but a retarget keeps the velocity, which toward a
        // nearer point is more than the spring can absorb. Measured before the fix: 44 px past.
        let start = Instant::now();
        let mut scroll = Motion::new(Animations::Full);
        scroll.to(Key::Scroll(0), 400.0, Spring::SCROLL, start);
        scroll.to(Key::Scroll(0), 130.0, Spring::SCROLL, at(start, 60));
        let beyond = (0..400)
            .map(|step| scroll.get(Key::Scroll(0), at(start, 60 + step * 5)))
            .fold(f32::MIN, f32::max);
        assert!(beyond <= 130.0 + 1e-3, "peaked at {beyond}");

        let mut reduced = Motion::new(Animations::Reduced);
        reduced.to(Key::Palette, 1.0, Spring::PANEL, start);
        reduced.to(Key::Palette, 0.274, Spring::PANEL, at(start, 75));
        let beyond = (0..400)
            .map(|step| reduced.get(Key::Palette, at(start, 75 + step * 5)))
            .fold(f32::MIN, f32::max);
        assert!(beyond <= 0.274 + 1e-3, "peaked at {beyond}");
    }

    #[test]
    fn momentum_away_from_the_new_target_is_kept() {
        // The clamp is for velocity toward the target only; reversing smoothly is why springs are used.
        let start = Instant::now();
        let mut motion = Motion::new(Animations::Reduced);
        motion.to(Key::Sidebar, 1.0, Spring::PANEL, start);
        let mid = at(start, 60);
        let before = motion.get(Key::Sidebar, mid);
        motion.to(Key::Sidebar, 0.0, Spring::PANEL, mid);
        assert!(motion.get(Key::Sidebar, at(start, 70)) > before);
    }

    #[test]
    fn a_degenerate_spring_still_comes_to_rest() {
        // A zero or NaN field would otherwise be an undamped or glacial spring, and `busy` would
        // keep the event loop awake for the rest of the session.
        let start = Instant::now();
        let mut motion = Motion::new(Animations::Full);
        let zero = Spring {
            stiffness: 0.0,
            damping_ratio: 0.0,
        };
        let nan = Spring {
            stiffness: f32::NAN,
            damping_ratio: f32::NAN,
        };
        motion.to(Key::Sidebar, 1.0, zero, start);
        motion.to(Key::Menu, 1.0, nan, start);
        assert!(!motion.busy(start + Duration::from_secs(900)));
    }

    #[test]
    fn retargeting_with_an_earlier_clock_does_not_rewind_the_timeline() {
        // Mixed time sources (a capture time against a real event time) must not make a value jump.
        let start = Instant::now();
        let mut motion = Motion::new(Animations::Full);
        motion.to(Key::Sidebar, 1.0, Spring::PANEL, at(start, 100));
        motion.to(Key::Sidebar, 1.0, Spring::PANEL, start);
        assert_eq!(motion.get(Key::Sidebar, at(start, 100)), 0.0);
    }

    #[test]
    fn asking_for_the_same_target_every_frame_changes_nothing() {
        // Draw code says where a panel should be on every frame. Restarting the spring each time
        // would re-anchor it at the current instant and slow or stall the move.
        let start = Instant::now();
        let mut once = Motion::new(Animations::Full);
        let mut every_frame = Motion::new(Animations::Full);
        once.to(Key::Sidebar, 1.0, Spring::PANEL, start);
        for frame in 0..200 {
            let now = at(start, frame * 16);
            every_frame.to(Key::Sidebar, 1.0, Spring::PANEL, now);
            assert_eq!(
                every_frame.get(Key::Sidebar, now),
                once.get(Key::Sidebar, now),
                "frame {frame}"
            );
            assert_eq!(every_frame.busy(now), once.busy(now), "frame {frame}");
        }
    }

    #[test]
    fn snapping_then_animating_from_an_earlier_clock_starts_at_that_clock() {
        // A capture renders "150 ms into the transition" from a time chosen by the caller, which can
        // precede the wall-clock instant of an earlier snap. The snap must not push the start later.
        let t0 = Instant::now() - Duration::from_secs(1);
        let mut motion = Motion::new(Animations::Full);
        motion.snap(Key::Sidebar, 0.0);
        motion.to(Key::Sidebar, 1.0, Spring::PANEL, t0);
        let mid = motion.get(Key::Sidebar, at(t0, 150));
        assert!(mid > 0.0 && mid < 1.0, "mid-flight {mid}");
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

        /// A spring that promises not to overshoot keeps the promise through any retargeting, not only
        /// from rest: the value never ends up on the far side of the target it was approaching.
        /// Every retarget is checked as if it were the last, which `get` being pure makes possible.
        #[test]
        fn a_no_overshoot_spring_never_crosses_its_target_after_any_retargets(
            steps in proptest::collection::vec((-1.0e3f32..1.0e3, 1u64..400), 2..=8),
            reduced in proptest::bool::ANY,
            spring in 0usize..SPRINGS.len(),
        ) {
            // Reduced makes every preset critical; Full only promises it for the critical ones.
            let (animations, spring) = if reduced {
                (Animations::Reduced, SPRINGS[spring])
            } else {
                (Animations::Full, [Spring::HOVER, Spring::SCROLL][spring % 2])
            };
            let start = Instant::now();
            let mut motion = Motion::new(animations);
            let mut clock = 0;
            for (target, gap) in steps {
                clock += gap;
                let now = at(start, clock);
                let before = motion.get(Key::Scroll(0), now);
                motion.to(Key::Scroll(0), target, spring, now);
                let scale = target.abs().max(1.0);
                // The side being approached means nothing if the value is already on the target.
                if (before - target).abs() > 1e-2 * scale {
                    let side = (before - target).signum();
                    for sample in 0..=400 {
                        let value = motion.get(Key::Scroll(0), at(start, clock + sample * 5));
                        prop_assert!(
                            (value - target) * side >= -1e-4 * scale,
                            "{value} crossed {target}, approaching from {side}"
                        );
                    }
                }
            }
        }
    }
}
