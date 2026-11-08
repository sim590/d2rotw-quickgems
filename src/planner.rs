//! Planificateur de transmutations.
//!
//! Une transmutation consomme 3 gemmes d'un palier et produit 1 gemme du
//! palier supérieur. On traite toujours le palier le plus bas en premier :
//! convertir un palier inférieur peut faire atteindre le seuil de 3 au palier
//! supérieur, alors que l'ordre inverse raterait des conversions.

/// Quantités par type de gemme (7) et par palier (5).
pub type Counts = [[u32; 5]; 7];
/// Quantités minimales à conserver, même forme que `Counts`.
pub type Keep = [[u32; 5]; 7];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Action {
    pub gem_type: usize,
    /// Palier source, 0..=3. La cible est `tier + 1`.
    pub tier: usize,
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub initial_counts: Counts,
    pub final_counts: Counts,
    pub actions: Vec<Action>,
    /// Nombre de conversions par type et par palier source.
    pub conversions: [[u32; 4]; 7],
}

/// Prochaine conversion valide. On balaie les paliers du bas vers le haut,
/// puis les types de gemmes.
pub fn next_action(counts: &Counts, keep: &Keep) -> Option<Action> {
    for tier in 0..4 {
        for gem_type in 0..7 {
            if counts[gem_type][tier] >= keep[gem_type][tier].saturating_add(3) {
                return Some(Action { gem_type, tier });
            }
        }
    }
    None
}

/// Projette toutes les conversions possibles à partir d'un état initial.
pub fn project(counts: Counts, keep: &Keep) -> Plan {
    project_limited(counts, keep, usize::MAX)
}

/// Projette au plus `max_actions` conversions. Sert à calculer un budget avant
/// de lancer un lot live.
pub fn project_limited(counts: Counts, keep: &Keep, max_actions: usize) -> Plan {
    let initial_counts = counts;
    let mut final_counts = counts;
    let mut actions = Vec::new();
    let mut conversions = [[0u32; 4]; 7];

    while actions.len() < max_actions {
        let Some(action) = next_action(&final_counts, keep) else {
            break;
        };
        final_counts[action.gem_type][action.tier] -= 3;
        final_counts[action.gem_type][action.tier + 1] += 1;
        conversions[action.gem_type][action.tier] += 1;
        actions.push(action);
    }

    Plan {
        initial_counts,
        final_counts,
        actions,
        conversions,
    }
}

impl Plan {
    pub fn conversion_count(&self) -> usize {
        self.actions.len()
    }

    pub fn perfect_gain(&self, gem_type: usize) -> u32 {
        self.final_counts[gem_type][4].saturating_sub(self.initial_counts[gem_type][4])
    }

    pub fn total_perfect_gain(&self) -> u32 {
        (0..7).map(|gem_type| self.perfect_gain(gem_type)).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_counts() -> Counts {
        [[0; 5]; 7]
    }

    fn empty_keep() -> Keep {
        [[0; 5]; 7]
    }

    #[test]
    fn refuses_to_go_below_keep() {
        let mut counts = empty_counts();
        let mut keep = empty_keep();
        counts[0][0] = 3;
        keep[0][0] = 1;
        assert_eq!(next_action(&counts, &keep), None);

        counts[0][0] = 4;
        assert_eq!(
            next_action(&counts, &keep),
            Some(Action {
                gem_type: 0,
                tier: 0
            })
        );
    }

    #[test]
    fn converts_bottom_up() {
        let mut counts = empty_counts();
        let mut keep = empty_keep();
        counts[0][0] = 3;
        counts[0][1] = 5;
        keep[0][1] = 3;

        let plan = project(counts, &keep);
        assert_eq!(plan.final_counts[0][0], 0);
        assert_eq!(plan.final_counts[0][1], 3);
        assert_eq!(plan.final_counts[0][2], 1);
        assert_eq!(plan.conversion_count(), 2);
        assert!(next_action(&plan.final_counts, &keep).is_none());
    }

    #[test]
    fn cascades_nine_to_one_higher_tier() {
        let mut counts = empty_counts();
        let keep = empty_keep();
        counts[0][0] = 9;

        let plan = project(counts, &keep);
        assert_eq!(plan.final_counts[0][0], 0);
        assert_eq!(plan.final_counts[0][1], 0);
        assert_eq!(plan.final_counts[0][2], 1);
        assert_eq!(plan.conversion_count(), 4);
        assert!(next_action(&plan.final_counts, &keep).is_none());
    }

    #[test]
    fn project_limited_stops_at_budget() {
        let mut counts = empty_counts();
        let keep = empty_keep();
        counts[0][0] = 9;

        let plan = project_limited(counts, &keep, 2);
        assert_eq!(plan.conversion_count(), 2);
        assert_eq!(plan.final_counts[0][0], 3);
        assert_eq!(plan.final_counts[0][1], 2);
    }

    #[test]
    fn plan_respects_every_keep_floor() {
        let mut counts = empty_counts();
        let mut keep = empty_keep();
        for gem_type in 0..7 {
            for tier in 0..5 {
                counts[gem_type][tier] = 12 + gem_type as u32 + tier as u32;
                keep[gem_type][tier] = 4 + (gem_type + tier) as u32 % 5;
            }
        }

        let initial = counts;
        let plan = project(counts, &keep);
        for gem_type in 0..7 {
            for tier in 0..5 {
                assert!(
                    plan.final_counts[gem_type][tier]
                        >= initial[gem_type][tier].min(keep[gem_type][tier]),
                    "le keep a été violé pour {gem_type}/{tier}"
                );
            }
        }
        assert!(next_action(&plan.final_counts, &keep).is_none());
    }
}
