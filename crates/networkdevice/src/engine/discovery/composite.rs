//! A primary backend plus a free neighbour-table baseline.
//!
//! Reading the kernel's neighbour table costs one file read: no packets, no
//! privileges, no external process. There is no reason to skip it, and real value
//! in always doing it, because it contributes whatever the kernel already learned
//! from unrelated traffic — a host that did not answer within the reply window, or
//! a device on an interface the active scan did not cover.
//!
//! So the concrete backends are composed rather than chosen:
//!
//! | selection | composition |
//! |---|---|
//! | `--backend neigh` | the neighbour backend alone — it *is* the baseline, plus a TCP sweep to populate it |
//! | `--backend pnet` | the in-process `arpscan` sweep **+** the neighbour baseline |
//!
//! The baseline never sweeps: it only reports what the kernel already knows. All
//! the packets come from the primary.
//!
//! Because the baseline is independent, a primary failure is no longer fatal. The
//! composite degrades to the baseline and reports the failure as a warning, which
//! is how a missing `CAP_NET_RAW` stays useful
//! instead of aborting the scan.

use std::sync::Arc;

use crate::engine::discovery::{Discovery, ScanOutcome};
use crate::engine::error::{Error, Result};
use crate::engine::model::DiscoverySource;

/// Runs a primary backend, then folds in the neighbour-table baseline.
pub struct CompositeDiscovery {
    primary: Arc<dyn Discovery>,
    baseline: Arc<dyn Discovery>,
}

impl CompositeDiscovery {
    pub fn new(primary: Arc<dyn Discovery>, baseline: Arc<dyn Discovery>) -> Self {
        CompositeDiscovery { primary, baseline }
    }

    /// The backend that actually puts packets on the wire.
    pub fn primary(&self) -> &Arc<dyn Discovery> {
        &self.primary
    }
}

impl Discovery for CompositeDiscovery {
    /// The primary's identity: that is the backend the user selected.
    ///
    /// Observations keep their own `source`, so baseline entries are still
    /// recorded as coming from the neighbour table.
    fn source(&self) -> DiscoverySource {
        self.primary.source()
    }

    fn describe(&self) -> String {
        format!("{} + neighbour baseline", self.primary.describe())
    }

    /// Always succeeds.
    ///
    /// The baseline works on any Linux host, so a scan can never come back empty
    /// for want of a privilege or a binary. Problems with the primary surface as
    /// warnings from [`Discovery::scan`] instead of stopping the scan up front —
    /// which is the point of having a fallback at all.
    fn preflight(&self) -> Result<()> {
        Ok(())
    }

    fn scan(&self) -> Result<ScanOutcome> {
        // A failed primary is a warning, not the end: the baseline may still have
        // something to say. The error text already names the backend, so it is
        // passed through rather than prefixed again.
        let mut outcome = match self.primary.scan() {
            Ok(outcome) => outcome,
            Err(error) => ScanOutcome::warning(Vec::new(), error.to_string()),
        };

        match self.baseline.scan() {
            Ok(baseline) => outcome.absorb(baseline),
            Err(error) => outcome
                .warnings
                .push(format!("neighbour-table baseline failed: {error}")),
        }

        // Nothing found and something went wrong: that is a real failure.
        // Finding nothing with no errors is just an empty network.
        if outcome.is_empty() && !outcome.warnings.is_empty() {
            return Err(Error::failed("scan", outcome.warnings.join("; ")));
        }

        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::model::{MacAddr, Observation};

    /// A backend that returns a canned outcome and never touches the network.
    struct Fixture {
        label: &'static str,
        source: DiscoverySource,
        result: std::result::Result<Vec<Observation>, String>,
    }

    impl Fixture {
        fn ok(label: &'static str, source: DiscoverySource, macs: &[&str]) -> Self {
            Fixture {
                label,
                source,
                result: Ok(macs.iter().map(|mac| observation(mac, source)).collect()),
            }
        }

        fn failing(label: &'static str, source: DiscoverySource, reason: &str) -> Self {
            Fixture {
                label,
                source,
                result: Err(reason.to_string()),
            }
        }
    }

    impl Discovery for Fixture {
        fn source(&self) -> DiscoverySource {
            self.source
        }
        fn describe(&self) -> String {
            self.label.to_string()
        }
        fn scan(&self) -> Result<ScanOutcome> {
            match &self.result {
                Ok(observations) => Ok(ScanOutcome::clean(observations.clone())),
                Err(reason) => Err(Error::failed(self.label, reason.clone())),
            }
        }
    }

    fn observation(mac: &str, source: DiscoverySource) -> Observation {
        Observation::from_mac(
            mac.parse::<MacAddr>().unwrap(),
            "10.0.0.1".parse().unwrap(),
            Some("eth0".into()),
        )
        .with_source(source)
    }

    fn composite(primary: Fixture, baseline: Fixture) -> CompositeDiscovery {
        CompositeDiscovery::new(Arc::new(primary), Arc::new(baseline))
    }

    #[test]
    fn merges_primary_and_baseline_observations() {
        let discovery = composite(
            Fixture::ok("primary", DiscoverySource::PnetArp, &["aa:bb:cc:00:00:01"]),
            Fixture::ok(
                "baseline",
                DiscoverySource::NeighborTable,
                &["aa:bb:cc:00:00:02"],
            ),
        );

        let outcome = discovery.scan().unwrap();
        assert_eq!(outcome.observations.len(), 2);
        assert!(outcome.warnings.is_empty());

        // Each observation keeps the source it came from.
        let sources: Vec<DiscoverySource> = outcome.observations.iter().map(|o| o.source).collect();
        assert!(sources.contains(&DiscoverySource::PnetArp));
        assert!(sources.contains(&DiscoverySource::NeighborTable));
    }

    #[test]
    fn a_failing_primary_degrades_to_the_baseline() {
        let discovery = composite(
            Fixture::failing("primary", DiscoverySource::PnetArp, "binary not found"),
            Fixture::ok(
                "baseline",
                DiscoverySource::NeighborTable,
                &["aa:bb:cc:00:00:01"],
            ),
        );

        let outcome = discovery.scan().unwrap();
        assert_eq!(outcome.observations.len(), 1, "baseline data must survive");
        assert_eq!(outcome.warnings.len(), 1);
        assert!(outcome.warnings[0].contains("binary not found"));
    }

    #[test]
    fn a_total_failure_is_still_an_error() {
        let discovery = composite(
            Fixture::failing("primary", DiscoverySource::PnetArp, "no privileges"),
            Fixture::failing(
                "baseline",
                DiscoverySource::NeighborTable,
                "no /proc/net/arp",
            ),
        );

        let error = discovery.scan().unwrap_err();
        let text = error.to_string();
        assert!(text.contains("no privileges"), "{text}");
        assert!(text.contains("no /proc/net/arp"), "{text}");
    }

    #[test]
    fn finding_nothing_without_errors_is_not_an_error() {
        let discovery = composite(
            Fixture::ok("primary", DiscoverySource::PnetArp, &[]),
            Fixture::ok("baseline", DiscoverySource::NeighborTable, &[]),
        );

        let outcome = discovery.scan().unwrap();
        assert!(outcome.is_empty());
        assert!(outcome.warnings.is_empty());
    }

    #[test]
    fn a_failing_baseline_does_not_lose_primary_results() {
        let discovery = composite(
            Fixture::ok("primary", DiscoverySource::PnetArp, &["aa:bb:cc:00:00:01"]),
            Fixture::failing(
                "baseline",
                DiscoverySource::NeighborTable,
                "no /proc/net/arp",
            ),
        );

        let outcome = discovery.scan().unwrap();
        assert_eq!(outcome.observations.len(), 1);
        assert_eq!(outcome.warnings.len(), 1);
        assert!(outcome.warnings[0].contains("baseline"));
    }

    #[test]
    fn preflight_never_blocks_a_scan() {
        // Even with a primary that cannot possibly run, the baseline means the
        // scan is worth attempting.
        let discovery = composite(
            Fixture::failing("primary", DiscoverySource::PnetArp, "no CAP_NET_RAW"),
            Fixture::ok("baseline", DiscoverySource::NeighborTable, &[]),
        );
        assert!(discovery.preflight().is_ok());
    }

    #[test]
    fn describe_names_both_halves_and_source_is_the_primary() {
        let discovery = composite(
            Fixture::ok("pnet", DiscoverySource::PnetArp, &[]),
            Fixture::ok("baseline", DiscoverySource::NeighborTable, &[]),
        );
        assert!(discovery.describe().contains("pnet"));
        assert!(discovery.describe().contains("neighbour baseline"));
        assert_eq!(discovery.source(), DiscoverySource::PnetArp);
    }
}
