use core::fmt::Write;
use core::time::Duration;
use std::collections::{BTreeMap, BTreeSet};
use std::string::String;
use std::vec::Vec;

use pump_shared::{
    Basal, Bolus, Extended, InsulinAmount, Percent, PumpMode, TherapyMachine, TherapyState,
    Transition,
};

const REGIONS: [&str; 4] = ["mode", "basal", "bolus", "extended"];

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Edge {
    pub region: &'static str,
    pub from: &'static str,
    pub transition: &'static str,
    pub to: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Requirement {
    pub transition: &'static str,
    pub region: &'static str,
    pub state: &'static str,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Graph {
    pub edges: BTreeSet<Edge>,
    pub requirements: BTreeSet<Requirement>,
}

impl Graph {
    pub fn standard() -> Self {
        Self::explore(|state| state)
    }

    pub fn explore<M: TherapyMachine>(make: impl Fn(TherapyState) -> M) -> Self {
        let mut edges = BTreeSet::new();
        let mut accepted: BTreeMap<&'static str, Vec<[&'static str; 4]>> = BTreeMap::new();
        for from in sample_states() {
            for transition in sample_transitions() {
                let mut machine = make(from);
                if machine.apply(transition).is_err() {
                    continue;
                }
                let to = *machine.therapy();
                let (before, after) = (labels(&from), labels(&to));
                accepted.entry(transition.name()).or_default().push(before);
                for (i, moved) in changed(&from, &to).into_iter().enumerate() {
                    if moved {
                        edges.insert(Edge {
                            region: REGIONS[i],
                            from: before[i],
                            transition: transition.name(),
                            to: after[i],
                        });
                    }
                }
            }
        }
        let requirements = accepted
            .iter()
            .flat_map(|(transition, froms)| {
                (0..REGIONS.len()).filter_map(move |i| {
                    let state = froms[0][i];
                    froms
                        .iter()
                        .all(|from| from[i] == state)
                        .then_some(Requirement {
                            transition,
                            region: REGIONS[i],
                            state,
                        })
                })
            })
            .collect();
        Self {
            edges,
            requirements,
        }
    }

    pub fn to_overview_mermaid(&self) -> String {
        let mut out = String::from("flowchart LR\n");
        let states = sample_states();
        for (i, region) in REGIONS.iter().enumerate() {
            let mut names: Vec<&str> = Vec::new();
            for state in &states {
                let name = labels(state)[i];
                if !names.contains(&name) {
                    names.push(name);
                }
            }
            let _ = writeln!(
                out,
                "    r_{region}[\"{region}<br/>{}\"]",
                names.join(" · ")
            );
        }
        for transition in sample_transitions().map(|t| t.name()) {
            let _ = writeln!(out, "    t_{transition}([{transition}])");
            for need in self
                .requirements
                .iter()
                .filter(|r| r.transition == transition)
            {
                let _ = writeln!(
                    out,
                    "    r_{} -. {} .-> t_{transition}",
                    need.region, need.state
                );
            }
            let touched: BTreeSet<_> = self
                .edges
                .iter()
                .filter(|e| e.transition == transition)
                .map(|e| e.region)
                .collect();
            for region in REGIONS.iter().filter(|r| touched.contains(*r)) {
                let _ = writeln!(out, "    t_{transition} --> r_{region}");
            }
        }
        out
    }

    pub fn to_mermaid(&self) -> String {
        let mut out = String::from("stateDiagram-v2\n    state Therapy {\n");
        for (i, region) in REGIONS.iter().enumerate() {
            if i > 0 {
                out.push_str("        --\n");
            }
            let _ = writeln!(out, "        state {region} {{");
            for edge in self.edges.iter().filter(|e| e.region == *region) {
                let _ = writeln!(
                    out,
                    "            {region}_{} --> {region}_{} : {}",
                    edge.from, edge.to, edge.transition
                );
            }
            out.push_str("        }\n");
        }
        out.push_str("    }\n");
        out
    }
}

fn units(hundredths: u32) -> InsulinAmount {
    InsulinAmount::from_hundredths(hundredths)
}

fn sample_states() -> Vec<TherapyState> {
    let hour = Duration::from_secs(3600);
    let modes = [
        PumpMode::Unknown,
        PumpMode::Off,
        PumpMode::Suspended,
        PumpMode::Running,
    ];
    let basals = [
        Basal::Unknown,
        Basal::Scheduled,
        Basal::Temporary {
            rate: Percent::new(150),
            duration: hour,
        },
    ];
    let boluses = [
        Bolus::Unknown,
        Bolus::Idle,
        Bolus::Immediate {
            requested: units(100),
            delivered: units(0),
        },
    ];
    let extendeds = [
        Extended::Unknown,
        Extended::Idle,
        Extended::Running {
            amount: units(100),
            duration: hour,
        },
    ];
    let mut states = Vec::new();
    for mode in modes {
        for basal in basals {
            for bolus in boluses {
                for extended in extendeds {
                    states.push(TherapyState {
                        mode,
                        basal,
                        bolus,
                        extended,
                    });
                }
            }
        }
    }
    states
}

fn sample_transitions() -> [Transition; 11] {
    let hour = Duration::from_secs(3600);
    [
        Transition::Suspended,
        Transition::Resumed,
        Transition::TurnedOff,
        Transition::BolusStarted {
            requested: units(100),
        },
        Transition::BolusProgressed {
            delivered: units(50),
        },
        Transition::BolusEnded,
        Transition::TempBasalStarted {
            rate: Percent::new(150),
            duration: hour,
        },
        Transition::TempBasalEnded,
        Transition::ExtendedStarted {
            amount: units(100),
            duration: hour,
        },
        Transition::ExtendedEnded,
        Transition::LinkLost,
    ]
}

fn labels(s: &TherapyState) -> [&'static str; 4] {
    [
        s.mode.name(),
        s.basal.name(),
        s.bolus.name(),
        s.extended.name(),
    ]
}

fn changed(a: &TherapyState, b: &TherapyState) -> [bool; 4] {
    [
        a.mode != b.mode,
        a.basal != b.basal,
        a.bolus != b.bolus,
        a.extended != b.extended,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge(
        region: &'static str,
        from: &'static str,
        transition: &'static str,
        to: &'static str,
    ) -> Edge {
        Edge {
            region,
            from,
            transition,
            to,
        }
    }

    #[test]
    fn standard_graph_has_the_main_edges() {
        let graph = Graph::standard();
        assert!(graph
            .edges
            .contains(&edge("bolus", "Idle", "BolusStarted", "Immediate")));
        assert!(graph
            .edges
            .contains(&edge("bolus", "Immediate", "BolusProgressed", "Immediate")));
        assert!(graph
            .edges
            .contains(&edge("mode", "Suspended", "Resumed", "Running")));
        assert!(graph
            .edges
            .contains(&edge("mode", "Running", "TurnedOff", "Off")));
        assert!(graph
            .edges
            .contains(&edge("basal", "Scheduled", "TempBasalStarted", "Temporary")));
        assert!(graph
            .edges
            .contains(&edge("extended", "Running", "LinkLost", "Unknown")));
    }

    #[test]
    fn exports_mermaid() {
        let graph = Graph::standard();
        let mermaid = graph.to_mermaid();
        assert!(mermaid.starts_with("stateDiagram-v2"));
        assert!(mermaid.contains("bolus_Idle --> bolus_Immediate : BolusStarted"));
    }

    struct NeverResumes(TherapyState);

    impl TherapyMachine for NeverResumes {
        fn therapy(&self) -> &TherapyState {
            &self.0
        }

        fn therapy_mut(&mut self) -> &mut TherapyState {
            &mut self.0
        }

        fn apply(&mut self, transition: Transition) -> Result<(), pump_shared::TransitionError> {
            if transition == Transition::Resumed {
                return self.0.apply(Transition::TurnedOff);
            }
            self.0.apply(transition)
        }
    }

    #[test]
    fn graph_follows_an_overridden_machine() {
        assert_eq!(
            NeverResumes(TherapyState::default()).therapy_mut().mode,
            PumpMode::Unknown
        );
        let graph = Graph::explore(NeverResumes);
        assert!(graph
            .edges
            .contains(&edge("mode", "Suspended", "Resumed", "Off")));
        assert!(!graph
            .edges
            .contains(&edge("mode", "Suspended", "Resumed", "Running")));
    }

    fn requirement(
        transition: &'static str,
        region: &'static str,
        state: &'static str,
    ) -> Requirement {
        Requirement {
            transition,
            region,
            state,
        }
    }

    #[test]
    fn finds_what_each_transition_requires() {
        let graph = Graph::standard();
        assert!(graph
            .requirements
            .contains(&requirement("BolusStarted", "mode", "Running")));
        assert!(graph
            .requirements
            .contains(&requirement("BolusStarted", "bolus", "Idle")));
        assert!(graph
            .requirements
            .contains(&requirement("Resumed", "mode", "Suspended")));
        assert!(!graph
            .requirements
            .iter()
            .any(|r| r.transition == "Suspended"));
    }

    #[test]
    fn overview_links_regions_and_transitions() {
        let overview = Graph::standard().to_overview_mermaid();
        assert!(overview.starts_with("flowchart LR"));
        assert!(overview.contains("r_mode[\"mode<br/>Unknown · Off · Suspended · Running\"]"));
        assert!(overview.contains("r_mode -. Running .-> t_BolusStarted"));
        assert!(overview.contains("t_BolusStarted --> r_bolus"));
    }

    #[test]
    fn exported_files_are_up_to_date() {
        assert_eq!(
            Graph::standard().to_overview_mermaid(),
            include_str!("../.docs/therapy-overview.mmd")
        );
        assert_eq!(
            Graph::standard().to_mermaid(),
            include_str!("../.docs/therapy.mmd")
        );
    }
}
