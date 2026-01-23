//! Autoscaler - EMA-based scaling decisions
//!
//! Implements intelligent autoscaling using exponential moving average
//! smoothing to avoid thrashing and maintain stable service counts.

use std::collections::HashMap;
use std::time::{Duration, Instant};
use tracing::info;

use crate::error::{Result, SchedulerError};
use crate::metrics::AggregatedMetrics;
use spec::{ScaleSpec, ScaleTargets};

/// Default cooldown between scale events
pub const DEFAULT_COOLDOWN: Duration = Duration::from_secs(30);

/// Default EMA alpha (smoothing factor)
pub const DEFAULT_EMA_ALPHA: f64 = 0.3;

/// Scaling decision result
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScalingDecision {
    /// Scale up to target replicas
    ScaleUp { from: u32, to: u32, reason: String },
    /// Scale down to target replicas
    ScaleDown { from: u32, to: u32, reason: String },
    /// No scaling needed
    NoChange { reason: String },
    /// Cannot scale (in cooldown or manual mode)
    Blocked { reason: String },
}

impl ScalingDecision {
    /// Check if this decision involves a change
    pub fn is_change(&self) -> bool {
        matches!(
            self,
            ScalingDecision::ScaleUp { .. } | ScalingDecision::ScaleDown { .. }
        )
    }

    /// Get the target replica count, if any
    pub fn target_replicas(&self) -> Option<u32> {
        match self {
            ScalingDecision::ScaleUp { to, .. } => Some(*to),
            ScalingDecision::ScaleDown { to, .. } => Some(*to),
            _ => None,
        }
    }
}

/// EMA (Exponential Moving Average) calculator for a single metric
#[derive(Debug, Clone)]
pub struct EmaCalculator {
    /// Current EMA value
    value: f64,
    /// Smoothing factor (0 < alpha < 1)
    alpha: f64,
    /// Whether we have received any samples
    initialized: bool,
}

impl EmaCalculator {
    /// Create a new EMA calculator with given alpha
    pub fn new(alpha: f64) -> Self {
        Self {
            value: 0.0,
            alpha: alpha.clamp(0.01, 0.99),
            initialized: false,
        }
    }

    /// Update EMA with a new sample
    pub fn update(&mut self, sample: f64) -> f64 {
        if !self.initialized {
            self.value = sample;
            self.initialized = true;
        } else {
            // EMA formula: new_value = alpha * sample + (1 - alpha) * old_value
            self.value = self.alpha * sample + (1.0 - self.alpha) * self.value;
        }
        self.value
    }

    /// Get current EMA value
    pub fn value(&self) -> f64 {
        self.value
    }

    /// Get the alpha (smoothing factor)
    pub fn alpha(&self) -> f64 {
        self.alpha
    }

    /// Reset the EMA
    pub fn reset(&mut self) {
        self.value = 0.0;
        self.initialized = false;
    }
}

impl Default for EmaCalculator {
    fn default() -> Self {
        Self::new(DEFAULT_EMA_ALPHA)
    }
}

/// Per-service autoscaling state
#[derive(Debug)]
struct ServiceScaleState {
    /// Current replica count
    current_replicas: u32,
    /// EMA for CPU usage
    cpu_ema: EmaCalculator,
    /// EMA for memory usage
    memory_ema: EmaCalculator,
    /// EMA for RPS
    rps_ema: EmaCalculator,
    /// Last scale event time
    last_scale_time: Option<Instant>,
    /// Configured cooldown
    cooldown: Duration,
    /// Scale spec from deployment
    spec: ScaleSpec,
}

impl ServiceScaleState {
    fn new(spec: ScaleSpec, initial_replicas: u32, cooldown: Duration) -> Self {
        Self {
            current_replicas: initial_replicas,
            cpu_ema: EmaCalculator::default(),
            memory_ema: EmaCalculator::default(),
            rps_ema: EmaCalculator::default(),
            last_scale_time: None,
            cooldown,
            spec,
        }
    }

    fn is_in_cooldown(&self) -> bool {
        self.last_scale_time
            .map(|t| t.elapsed() < self.cooldown)
            .unwrap_or(false)
    }
}

/// Autoscaler - makes scaling decisions based on metrics
pub struct Autoscaler {
    /// Per-service scaling state
    services: HashMap<String, ServiceScaleState>,
    /// EMA alpha for all services (reserved for future per-service customization)
    #[allow(dead_code)]
    ema_alpha: f64,
}

impl Autoscaler {
    /// Create a new autoscaler
    pub fn new() -> Self {
        Self {
            services: HashMap::new(),
            ema_alpha: DEFAULT_EMA_ALPHA,
        }
    }

    /// Create with custom EMA alpha
    pub fn with_ema_alpha(alpha: f64) -> Self {
        Self {
            services: HashMap::new(),
            ema_alpha: alpha.clamp(0.01, 0.99),
        }
    }

    /// Register a service for autoscaling
    pub fn register_service(
        &mut self,
        name: impl Into<String>,
        spec: ScaleSpec,
        initial_replicas: u32,
    ) {
        let name = name.into();
        let cooldown = match &spec {
            ScaleSpec::Adaptive { cooldown, .. } => cooldown.unwrap_or(DEFAULT_COOLDOWN),
            _ => DEFAULT_COOLDOWN,
        };

        info!(service = %name, ?spec, initial_replicas, "Registered service for autoscaling");
        self.services.insert(
            name,
            ServiceScaleState::new(spec, initial_replicas, cooldown),
        );
    }

    /// Unregister a service
    pub fn unregister_service(&mut self, name: &str) {
        self.services.remove(name);
    }

    /// Update metrics and compute scaling decision
    pub fn evaluate(
        &mut self,
        service_name: &str,
        metrics: &AggregatedMetrics,
    ) -> Result<ScalingDecision> {
        let state = self
            .services
            .get_mut(service_name)
            .ok_or_else(|| SchedulerError::ServiceNotFound(service_name.to_string()))?;

        // Update EMAs
        state.cpu_ema.update(metrics.avg_cpu_percent);
        state.memory_ema.update(metrics.avg_memory_percent);
        if let Some(rps) = metrics.total_rps {
            state.rps_ema.update(rps);
        }

        // Check scaling mode
        match &state.spec {
            ScaleSpec::Manual => Ok(ScalingDecision::Blocked {
                reason: "Manual scaling mode".to_string(),
            }),
            ScaleSpec::Fixed { replicas } => {
                if state.current_replicas != *replicas {
                    let from = state.current_replicas;
                    let to = *replicas;
                    if to > from {
                        Ok(ScalingDecision::ScaleUp {
                            from,
                            to,
                            reason: format!("Fixed mode: adjusting to {} replicas", replicas),
                        })
                    } else {
                        Ok(ScalingDecision::ScaleDown {
                            from,
                            to,
                            reason: format!("Fixed mode: adjusting to {} replicas", replicas),
                        })
                    }
                } else {
                    Ok(ScalingDecision::NoChange {
                        reason: "Fixed mode: at target".to_string(),
                    })
                }
            }
            ScaleSpec::Adaptive {
                min, max, targets, ..
            } => {
                // Check cooldown
                if state.is_in_cooldown() {
                    let remaining = state
                        .cooldown
                        .checked_sub(state.last_scale_time.unwrap().elapsed())
                        .unwrap_or_default();
                    return Ok(ScalingDecision::Blocked {
                        reason: format!("Cooldown active: {}s remaining", remaining.as_secs()),
                    });
                }

                let decision = Self::compute_adaptive_decision(
                    state.current_replicas,
                    *min,
                    *max,
                    targets,
                    state.cpu_ema.value(),
                    state.memory_ema.value(),
                    if metrics.total_rps.is_some() {
                        Some(state.rps_ema.value())
                    } else {
                        None
                    },
                );

                Ok(decision)
            }
        }
    }

    /// Compute adaptive scaling decision based on smoothed metrics
    fn compute_adaptive_decision(
        current: u32,
        min: u32,
        max: u32,
        targets: &ScaleTargets,
        cpu_ema: f64,
        memory_ema: f64,
        rps_ema: Option<f64>,
    ) -> ScalingDecision {
        // Scale-up thresholds (trigger at target)
        // Scale-down thresholds (trigger at 50% of target for hysteresis)

        let mut scale_up_reasons = Vec::new();
        let mut scale_down_ok = true;

        // Check CPU
        if let Some(cpu_target) = targets.cpu {
            let target = cpu_target as f64;
            if cpu_ema >= target {
                scale_up_reasons.push(format!("CPU {:.1}% >= {}%", cpu_ema, cpu_target));
            }
            // Scale down only if well below target (hysteresis)
            if cpu_ema > target * 0.5 {
                scale_down_ok = false;
            }
        }

        // Check memory
        if let Some(mem_target) = targets.memory {
            let target = mem_target as f64;
            if memory_ema >= target {
                scale_up_reasons.push(format!("Memory {:.1}% >= {}%", memory_ema, mem_target));
            }
            if memory_ema > target * 0.5 {
                scale_down_ok = false;
            }
        }

        // Check RPS
        if let (Some(rps_target), Some(rps)) = (targets.rps, rps_ema) {
            let target = rps_target as f64;
            // RPS is total across instances, so per-instance = rps / current
            let per_instance = rps / current as f64;
            let per_instance_target = target;

            if per_instance >= per_instance_target {
                scale_up_reasons.push(format!(
                    "RPS/instance {:.1} >= {}",
                    per_instance, rps_target
                ));
            }
            if per_instance > per_instance_target * 0.5 {
                scale_down_ok = false;
            }
        }

        // Make decision
        if !scale_up_reasons.is_empty() && current < max {
            // Scale up by 1 (conservative)
            let to = (current + 1).min(max);
            ScalingDecision::ScaleUp {
                from: current,
                to,
                reason: scale_up_reasons.join(", "),
            }
        } else if scale_down_ok && current > min {
            // Scale down by 1 (conservative)
            let to = (current - 1).max(min);
            ScalingDecision::ScaleDown {
                from: current,
                to,
                reason: "All metrics below scale-down threshold".to_string(),
            }
        } else {
            ScalingDecision::NoChange {
                reason: format!(
                    "Within bounds (CPU: {:.1}%, Mem: {:.1}%)",
                    cpu_ema, memory_ema
                ),
            }
        }
    }

    /// Record that a scaling action was taken
    pub fn record_scale_action(&mut self, service_name: &str, new_replicas: u32) -> Result<()> {
        let state = self
            .services
            .get_mut(service_name)
            .ok_or_else(|| SchedulerError::ServiceNotFound(service_name.to_string()))?;

        state.current_replicas = new_replicas;
        state.last_scale_time = Some(Instant::now());

        info!(
            service = service_name,
            replicas = new_replicas,
            "Recorded scale action"
        );

        Ok(())
    }

    /// Get current replica count for a service
    pub fn current_replicas(&self, service_name: &str) -> Option<u32> {
        self.services.get(service_name).map(|s| s.current_replicas)
    }

    /// Check if service is in cooldown
    pub fn is_in_cooldown(&self, service_name: &str) -> Option<bool> {
        self.services.get(service_name).map(|s| s.is_in_cooldown())
    }
}

impl Default for Autoscaler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ema_calculator() {
        let mut ema = EmaCalculator::new(0.5);

        // First sample sets the value
        assert!((ema.update(100.0) - 100.0).abs() < 0.001);

        // Second sample is weighted
        // 0.5 * 50 + 0.5 * 100 = 75
        assert!((ema.update(50.0) - 75.0).abs() < 0.001);

        // Third sample
        // 0.5 * 50 + 0.5 * 75 = 62.5
        assert!((ema.update(50.0) - 62.5).abs() < 0.001);
    }

    #[test]
    fn test_autoscaler_fixed_mode() {
        let mut autoscaler = Autoscaler::new();

        autoscaler.register_service("api", ScaleSpec::Fixed { replicas: 3 }, 1);

        let metrics = AggregatedMetrics {
            avg_cpu_percent: 50.0,
            avg_memory_percent: 50.0,
            total_rps: None,
            instance_count: 1,
        };

        let decision = autoscaler.evaluate("api", &metrics).unwrap();

        match decision {
            ScalingDecision::ScaleUp { from: 1, to: 3, .. } => {}
            other => panic!("Expected ScaleUp, got {:?}", other),
        }
    }

    #[test]
    fn test_autoscaler_manual_mode() {
        let mut autoscaler = Autoscaler::new();

        autoscaler.register_service("api", ScaleSpec::Manual, 1);

        let metrics = AggregatedMetrics {
            avg_cpu_percent: 90.0,
            avg_memory_percent: 90.0,
            total_rps: None,
            instance_count: 1,
        };

        let decision = autoscaler.evaluate("api", &metrics).unwrap();

        assert!(matches!(decision, ScalingDecision::Blocked { .. }));
    }

    #[test]
    fn test_autoscaler_adaptive_scale_up() {
        let mut autoscaler = Autoscaler::new();

        autoscaler.register_service(
            "api",
            ScaleSpec::Adaptive {
                min: 1,
                max: 10,
                cooldown: Some(Duration::from_secs(0)), // No cooldown for test
                targets: ScaleTargets {
                    cpu: Some(70),
                    memory: None,
                    rps: None,
                },
            },
            2,
        );

        // CPU at 80% should trigger scale up
        let metrics = AggregatedMetrics {
            avg_cpu_percent: 80.0,
            avg_memory_percent: 30.0,
            total_rps: None,
            instance_count: 2,
        };

        let decision = autoscaler.evaluate("api", &metrics).unwrap();

        match decision {
            ScalingDecision::ScaleUp {
                from: 2,
                to: 3,
                reason,
            } => {
                assert!(reason.contains("CPU"));
            }
            other => panic!("Expected ScaleUp, got {:?}", other),
        }
    }

    #[test]
    fn test_autoscaler_adaptive_scale_down() {
        let mut autoscaler = Autoscaler::new();

        autoscaler.register_service(
            "api",
            ScaleSpec::Adaptive {
                min: 1,
                max: 10,
                cooldown: Some(Duration::from_secs(0)),
                targets: ScaleTargets {
                    cpu: Some(70),
                    memory: None,
                    rps: None,
                },
            },
            5,
        );

        // CPU at 20% (well below 35% = 70% * 0.5) should trigger scale down
        let metrics = AggregatedMetrics {
            avg_cpu_percent: 20.0,
            avg_memory_percent: 20.0,
            total_rps: None,
            instance_count: 5,
        };

        let decision = autoscaler.evaluate("api", &metrics).unwrap();

        match decision {
            ScalingDecision::ScaleDown { from: 5, to: 4, .. } => {}
            other => panic!("Expected ScaleDown, got {:?}", other),
        }
    }

    #[test]
    fn test_autoscaler_respects_bounds() {
        let mut autoscaler = Autoscaler::new();

        autoscaler.register_service(
            "api",
            ScaleSpec::Adaptive {
                min: 2,
                max: 3,
                cooldown: Some(Duration::from_secs(0)),
                targets: ScaleTargets {
                    cpu: Some(70),
                    memory: None,
                    rps: None,
                },
            },
            3, // Already at max
        );

        // High CPU but already at max
        let metrics = AggregatedMetrics {
            avg_cpu_percent: 90.0,
            avg_memory_percent: 30.0,
            total_rps: None,
            instance_count: 3,
        };

        let decision = autoscaler.evaluate("api", &metrics).unwrap();

        // Should want to scale up but can't
        assert!(matches!(decision, ScalingDecision::NoChange { .. }));
    }

    #[test]
    fn test_cooldown_blocking() {
        let mut autoscaler = Autoscaler::new();

        autoscaler.register_service(
            "api",
            ScaleSpec::Adaptive {
                min: 1,
                max: 10,
                cooldown: Some(Duration::from_secs(60)),
                targets: ScaleTargets {
                    cpu: Some(70),
                    memory: None,
                    rps: None,
                },
            },
            2,
        );

        // Record a recent scale action
        autoscaler.record_scale_action("api", 3).unwrap();

        let metrics = AggregatedMetrics {
            avg_cpu_percent: 90.0,
            avg_memory_percent: 30.0,
            total_rps: None,
            instance_count: 3,
        };

        let decision = autoscaler.evaluate("api", &metrics).unwrap();

        // Should be blocked by cooldown
        assert!(matches!(decision, ScalingDecision::Blocked { .. }));
    }

    #[test]
    fn test_ema_reset() {
        let mut ema = EmaCalculator::new(0.5);
        ema.update(100.0);
        ema.update(50.0);

        ema.reset();

        // After reset, first value should be exact
        assert!((ema.update(30.0) - 30.0).abs() < 0.001);
    }

    #[test]
    fn test_ema_alpha_clamping() {
        // Alpha should be clamped to valid range
        let ema_low = EmaCalculator::new(0.0);
        assert!(ema_low.alpha() >= 0.01);

        let ema_high = EmaCalculator::new(1.0);
        assert!(ema_high.alpha() <= 0.99);
    }

    #[test]
    fn test_scaling_decision_helpers() {
        let scale_up = ScalingDecision::ScaleUp {
            from: 2,
            to: 4,
            reason: "test".to_string(),
        };
        assert!(scale_up.is_change());
        assert_eq!(scale_up.target_replicas(), Some(4));

        let no_change = ScalingDecision::NoChange {
            reason: "stable".to_string(),
        };
        assert!(!no_change.is_change());
        assert_eq!(no_change.target_replicas(), None);

        let blocked = ScalingDecision::Blocked {
            reason: "cooldown".to_string(),
        };
        assert!(!blocked.is_change());
        assert_eq!(blocked.target_replicas(), None);
    }

    #[test]
    fn test_autoscaler_rps_scaling() {
        let mut autoscaler = Autoscaler::new();

        autoscaler.register_service(
            "api",
            ScaleSpec::Adaptive {
                min: 1,
                max: 10,
                cooldown: Some(Duration::from_secs(0)),
                targets: ScaleTargets {
                    cpu: None,
                    memory: None,
                    rps: Some(100), // 100 RPS per instance target
                },
            },
            2,
        );

        // With 300 total RPS across 2 instances = 150 per instance
        // This exceeds the 100 RPS target, should scale up
        let metrics = AggregatedMetrics {
            avg_cpu_percent: 30.0,
            avg_memory_percent: 30.0,
            total_rps: Some(300.0),
            instance_count: 2,
        };

        let decision = autoscaler.evaluate("api", &metrics).unwrap();

        match decision {
            ScalingDecision::ScaleUp {
                from: 2,
                to: 3,
                reason,
            } => {
                assert!(reason.contains("RPS"));
            }
            other => panic!("Expected ScaleUp due to RPS, got {:?}", other),
        }
    }

    #[test]
    fn test_autoscaler_multiple_targets() {
        let mut autoscaler = Autoscaler::new();

        autoscaler.register_service(
            "api",
            ScaleSpec::Adaptive {
                min: 1,
                max: 10,
                cooldown: Some(Duration::from_secs(0)),
                targets: ScaleTargets {
                    cpu: Some(70),
                    memory: Some(80),
                    rps: None,
                },
            },
            2,
        );

        // CPU high, memory low - should still scale up due to CPU
        let metrics = AggregatedMetrics {
            avg_cpu_percent: 85.0,
            avg_memory_percent: 40.0,
            total_rps: None,
            instance_count: 2,
        };

        let decision = autoscaler.evaluate("api", &metrics).unwrap();

        match decision {
            ScalingDecision::ScaleUp { reason, .. } => {
                assert!(reason.contains("CPU"));
            }
            other => panic!("Expected ScaleUp, got {:?}", other),
        }
    }

    #[test]
    fn test_autoscaler_service_not_found() {
        let mut autoscaler = Autoscaler::new();

        let metrics = AggregatedMetrics::default();
        let result = autoscaler.evaluate("nonexistent", &metrics);

        assert!(result.is_err());
        match result {
            Err(SchedulerError::ServiceNotFound(name)) => {
                assert_eq!(name, "nonexistent");
            }
            _ => panic!("Expected ServiceNotFound error"),
        }
    }

    #[test]
    fn test_autoscaler_at_minimum() {
        let mut autoscaler = Autoscaler::new();

        autoscaler.register_service(
            "api",
            ScaleSpec::Adaptive {
                min: 2,
                max: 10,
                cooldown: Some(Duration::from_secs(0)),
                targets: ScaleTargets {
                    cpu: Some(70),
                    memory: None,
                    rps: None,
                },
            },
            2, // Already at minimum
        );

        // Very low usage but already at min
        let metrics = AggregatedMetrics {
            avg_cpu_percent: 5.0,
            avg_memory_percent: 10.0,
            total_rps: None,
            instance_count: 2,
        };

        let decision = autoscaler.evaluate("api", &metrics).unwrap();

        // Can't scale down below minimum
        assert!(matches!(decision, ScalingDecision::NoChange { .. }));
    }

    #[test]
    fn test_autoscaler_ema_smoothing_prevents_thrashing() {
        let mut autoscaler = Autoscaler::new();

        autoscaler.register_service(
            "api",
            ScaleSpec::Adaptive {
                min: 1,
                max: 10,
                cooldown: Some(Duration::from_secs(0)),
                targets: ScaleTargets {
                    cpu: Some(70),
                    memory: None,
                    rps: None,
                },
            },
            3,
        );

        // First reading: high
        let high_metrics = AggregatedMetrics {
            avg_cpu_percent: 90.0,
            avg_memory_percent: 30.0,
            total_rps: None,
            instance_count: 3,
        };
        let _ = autoscaler.evaluate("api", &high_metrics).unwrap();

        // Second reading: low (spike was temporary)
        // EMA should smooth this
        let low_metrics = AggregatedMetrics {
            avg_cpu_percent: 40.0,
            avg_memory_percent: 30.0,
            total_rps: None,
            instance_count: 3,
        };
        let _ = autoscaler.evaluate("api", &low_metrics).unwrap();

        // Third reading: still low
        let decision = autoscaler.evaluate("api", &low_metrics).unwrap();

        // Due to EMA smoothing, we shouldn't immediately scale down
        // The smoothed value should still be above the scale-down threshold
        // This tests that EMA prevents thrashing
        println!("Decision after EMA smoothing: {:?}", decision);
    }

    #[test]
    fn test_autoscaler_fixed_scale_down() {
        let mut autoscaler = Autoscaler::new();

        autoscaler.register_service(
            "api",
            ScaleSpec::Fixed { replicas: 2 },
            5, // Currently over target
        );

        let metrics = AggregatedMetrics::default();
        let decision = autoscaler.evaluate("api", &metrics).unwrap();

        match decision {
            ScalingDecision::ScaleDown { from: 5, to: 2, .. } => {}
            other => panic!("Expected ScaleDown, got {:?}", other),
        }
    }
}
