/// Attack, decay, sustain and release settings. Times are in seconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AdsrParams {
    /// Time to rise from 0 to 1.
    pub attack: f32,
    /// Time to fall from 1 to the sustain level.
    pub decay: f32,
    /// Level held while the gate is on, 0..=1.
    pub sustain: f32,
    /// Time to fall from the current level to 0 after the gate closes.
    pub release: f32,
    /// Shape of the decay and release segments.
    pub curve: Curve,
}

/// Shape of the decay and release segments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Curve {
    /// Straight lines.
    #[default]
    Linear,
    /// Convex fades that fall fast and then level off, like a sampled
    /// instrument's volume envelope: the level follows a straight line in
    /// decibels, so a plucked string rings out instead of hanging.
    Exponential,
}

impl Default for AdsrParams {
    fn default() -> Self {
        Self {
            attack: 0.002,
            decay: 0.3,
            sustain: 0.8,
            release: 0.12,
            curve: Curve::Linear,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

/// Minimum release so that note-offs never click.
const MIN_RELEASE_SECONDS: f32 = 0.001;

/// A linear-segment ADSR envelope generator.
#[derive(Debug, Clone)]
pub struct Adsr {
    stage: Stage,
    level: f32,
    release_step: f32,
    sample_rate: f32,
    params: AdsrParams,
}

impl Adsr {
    /// Creates an idle envelope.
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        Self {
            stage: Stage::Idle,
            level: 0.0,
            release_step: 0.0,
            sample_rate: sample_rate.max(1.0),
            params: AdsrParams::default(),
        }
    }

    /// Replaces the settings. Takes effect from the next sample.
    pub fn set_params(&mut self, params: AdsrParams) {
        self.params = params;
    }

    /// Opens the gate: starts the attack from the current level.
    pub fn gate_on(&mut self) {
        self.stage = Stage::Attack;
    }

    /// Closes the gate: starts the release from the current level.
    pub fn gate_off(&mut self) {
        if self.stage != Stage::Idle {
            let seconds = self.params.release.max(MIN_RELEASE_SECONDS);
            self.release_step = self.level / (seconds * self.sample_rate);
            self.stage = Stage::Release;
        }
    }

    /// Silences immediately.
    pub fn reset(&mut self) {
        self.stage = Stage::Idle;
        self.level = 0.0;
    }

    /// Whether the envelope has finished (output is zero and stays zero).
    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.stage == Stage::Idle
    }

    /// Whether the gate has been closed.
    #[must_use]
    pub fn is_releasing(&self) -> bool {
        self.stage == Stage::Release
    }

    /// Current level without advancing.
    #[must_use]
    pub fn level(&self) -> f32 {
        self.level
    }

    /// Advances one sample and returns the level.
    pub fn next_level(&mut self) -> f32 {
        let p = self.params;
        let sustain = p.sustain.clamp(0.0, 1.0);
        match self.stage {
            Stage::Idle => {}
            Stage::Attack => {
                if p.attack <= 0.0 {
                    self.level = 1.0;
                } else {
                    self.level += 1.0 / (p.attack * self.sample_rate);
                }
                if self.level >= 1.0 {
                    self.level = 1.0;
                    self.stage = Stage::Decay;
                }
            }
            Stage::Decay => {
                if p.decay <= 0.0 {
                    self.level = sustain;
                } else if p.curve == Curve::Exponential && self.level > sustain {
                    if sustain > 0.0 {
                        // A straight line in decibels: the level reaches the
                        // sustain exactly when the decay time is up.
                        self.level *= sustain.powf(1.0 / (p.decay * self.sample_rate));
                    } else {
                        self.level *= 10.0_f32.powf(-3.0 / (p.decay * self.sample_rate));
                        if self.level <= 1e-3 {
                            self.level = 0.0;
                            self.stage = Stage::Sustain;
                        }
                    }
                } else {
                    self.level -= (1.0 - sustain) / (p.decay * self.sample_rate);
                }
                if self.level <= sustain {
                    self.level = sustain;
                    self.stage = Stage::Sustain;
                }
            }
            Stage::Sustain => self.level = sustain,
            Stage::Release => {
                if p.curve == Curve::Exponential {
                    let seconds = p.release.max(MIN_RELEASE_SECONDS);
                    self.level *= 10.0_f32.powf(-3.0 / (seconds * self.sample_rate));
                    if self.level <= 1e-3 {
                        self.level = 0.0;
                        self.stage = Stage::Idle;
                    }
                } else {
                    self.level -= self.release_step.max(1e-9);
                    if self.level <= 0.0 {
                        self.level = 0.0;
                        self.stage = Stage::Idle;
                    }
                }
            }
        }
        self.level
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(params: AdsrParams) -> Adsr {
        let mut env = Adsr::new(1_000.0);
        env.set_params(params);
        env
    }

    #[test]
    fn walks_through_every_stage() {
        let mut e = env(AdsrParams {
            attack: 0.01,
            decay: 0.01,
            sustain: 0.5,
            release: 0.01,
            curve: Curve::Linear,
        });
        assert!(e.is_idle());
        e.gate_on();
        for _ in 0..10 {
            e.next_level();
        }
        assert!((e.level() - 1.0).abs() < 1e-5);
        for _ in 0..10 {
            e.next_level();
        }
        assert!((e.level() - 0.5).abs() < 1e-5);
        for _ in 0..100 {
            e.next_level();
        }
        assert!((e.level() - 0.5).abs() < 1e-5);
        e.gate_off();
        assert!(e.is_releasing());
        for _ in 0..11 {
            e.next_level();
        }
        assert!(e.is_idle());
        assert!(e.level().abs() < f32::EPSILON);
    }

    #[test]
    fn zero_times_jump_immediately() {
        let mut e = env(AdsrParams {
            attack: 0.0,
            decay: 0.0,
            sustain: 0.25,
            release: 0.0,
            curve: Curve::Linear,
        });
        e.gate_on();
        assert!((e.next_level() - 1.0).abs() < f32::EPSILON);
        assert!((e.next_level() - 0.25).abs() < f32::EPSILON);
        e.gate_off();
        // Minimum release of 1 ms = 1 sample at 1 kHz.
        e.next_level();
        assert!(e.is_idle());
    }

    #[test]
    fn gate_off_while_idle_stays_idle() {
        let mut e = env(AdsrParams::default());
        e.gate_off();
        assert!(e.is_idle());
    }

    #[test]
    fn exponential_decay_falls_faster_than_linear() {
        let params = AdsrParams {
            attack: 0.0,
            decay: 1.0,
            sustain: 0.01,
            release: 0.05,
            curve: Curve::Exponential,
        };
        let mut exponential = env(params);
        let mut linear = env(AdsrParams {
            curve: Curve::Linear,
            ..params
        });
        exponential.gate_on();
        linear.gate_on();
        for _ in 0..250 {
            exponential.next_level();
            linear.next_level();
        }
        // A straight line in decibels: 0.01^(1/4) after a quarter of the decay.
        let expected = 0.01_f32.powf(0.25);
        assert!((exponential.level() - expected).abs() < 0.01);
        assert!(exponential.level() < 0.35);
        assert!(linear.level() > 0.7);
        for _ in 0..750 {
            exponential.next_level();
        }
        assert!((exponential.level() - 0.01).abs() < 1e-4);
        assert!(!exponential.is_releasing());
    }

    #[test]
    fn exponential_decay_without_sustain_fades_out() {
        let mut e = env(AdsrParams {
            attack: 0.0,
            decay: 1.0,
            sustain: 0.0,
            release: 0.05,
            curve: Curve::Exponential,
        });
        e.gate_on();
        for _ in 0..1_100 {
            e.next_level();
        }
        assert!(e.level().abs() < f32::EPSILON);
    }

    #[test]
    fn exponential_release_fades_out_and_goes_idle() {
        let mut e = env(AdsrParams {
            attack: 0.0,
            decay: 0.0,
            sustain: 0.01,
            release: 0.05,
            curve: Curve::Exponential,
        });
        e.gate_on();
        e.next_level();
        e.gate_off();
        assert!(e.is_releasing());
        for _ in 0..50 {
            e.next_level();
        }
        assert!(e.is_idle());
        assert!(e.level().abs() < f32::EPSILON);
    }
}
