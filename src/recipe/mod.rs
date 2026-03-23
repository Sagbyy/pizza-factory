pub mod parser;
pub use parser::{flatten_recipe, parse_recipes, ParseError};

use crate::protocol::ActionDef;

/// A named pizza recipe parsed from a .recipes file or received via `recipe_answer`.
///
/// `source` is the canonical single-line DSL representation, ready to be sent
/// verbatim in a `RecipeAnswer` message without any further transformation.
///
/// Example source: `"Pepperoni = MakeDough -> AddBase(base_type=tomato) -> Bake(duration=6)"`
#[derive(Debug, Clone)]
pub struct Recipe {
    pub name: String,
    pub steps: Vec<Step>,
    /// Canonical single-line DSL string — the source of truth for wire transmission.
    /// Always regenerated from `steps` after parsing so it is normalised regardless
    /// of whether the recipe was loaded from a multi-line file or a flat wire string.
    pub source: String,
}

impl Recipe {
    /// Build the canonical single-line DSL representation from the parsed steps.
    ///
    /// Input:  `Recipe { name: "Pepperoni", steps: [Single(MakeDough), Single(AddBase{tomato})] }`
    /// Output: `"Pepperoni = MakeDough -> AddBase(base_type=tomato)"`
    ///
    /// This is the value sent in `RecipeAnswer.recipe` over the wire.
    pub fn to_dsl_string(&self) -> String {
        let steps_str = self
            .steps
            .iter()
            .map(step_to_dsl)
            .collect::<Vec<_>>()
            .join(" -> ");
        format!("{} = {}", self.name, steps_str)
    }
}

// ── DSL serialisation helpers (used by to_dsl_string) ────────────────────────

fn step_to_dsl(step: &Step) -> String {
    match step {
        Step::Single(a) => action_to_dsl(a),
        Step::Parallel(actions) => {
            let inner = actions.iter().map(action_to_dsl).collect::<Vec<_>>().join(", ");
            format!("[{inner}]")
        }
        Step::Repeated(a, n) => format!("{}^{n}", action_to_dsl(a)),
    }
}

/// Serialise an `ActionDef` back to DSL text.
/// Params are emitted in sorted key order for deterministic output.
///
/// Input:  `ActionDef { name: "AddBase", params: {"base_type": "tomato"} }`
/// Output: `"AddBase(base_type=tomato)"`
///
/// Input:  `ActionDef { name: "MakeDough", params: {} }`
/// Output: `"MakeDough"`
fn action_to_dsl(action: &ActionDef) -> String {
    if action.params.is_empty() {
        return action.name.clone();
    }
    let mut pairs: Vec<(&String, &String)> = action.params.iter().collect();
    pairs.sort_by_key(|(k, _)| *k);
    let params = pairs
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(",");
    format!("{}({params})", action.name)
}

/// One step in a recipe.
///
/// Steps are sequential. Parallel groups both actions in the same step.
/// Repeated expands the same action N times when flattened.
#[derive(Debug, Clone)]
pub enum Step {
    /// A single action: `AddBase(base_type=tomato)`
    Single(ActionDef),
    /// Two or more concurrent actions: `[AddCheese(amount=2), AddBasil(leaves=3)]`
    Parallel(Vec<ActionDef>),
    /// The same action repeated N times: `AddCheese(amount=1)^4`
    Repeated(ActionDef, u32),
}
