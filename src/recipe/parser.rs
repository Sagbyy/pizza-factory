#![allow(dead_code)]

use std::collections::HashMap;

use crate::protocol::ActionDef;
use crate::recipe::{Recipe, Step};

// ── Error type ────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum ParseError {
    /// A recipe block did not contain a `=` separating name from steps.
    MissingEquals,
    /// The recipe name (left of `=`) was empty.
    EmptyName,
    /// A recipe block had a name but contained no step lines.
    NoSteps { recipe: String },
    /// A parallel step `[...]` had unmatched or missing brackets.
    UnmatchedBracket { step: usize },
    /// A repeat suffix `^N` was missing or non-positive.
    InvalidRepeat { step: usize },
    /// A parameter token did not contain `=`.
    MalformedParam { token: String },
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::MissingEquals => write!(f, "recipe block is missing '='"),
            ParseError::EmptyName => write!(f, "recipe name is empty"),
            ParseError::NoSteps { recipe } => write!(f, "recipe '{recipe}' has no steps"),
            ParseError::UnmatchedBracket { step } => {
                write!(f, "step {step}: unmatched '[' or ']' in parallel step")
            }
            ParseError::InvalidRepeat { step } => {
                write!(f, "step {step}: invalid or zero repeat count after '^'")
            }
            ParseError::MalformedParam { token } => {
                write!(f, "malformed parameter '{token}': expected key=value")
            }
        }
    }
}

impl std::error::Error for ParseError {}

// ── Public API ────────────────────────────────────────────────────────────────

/// Parse all recipes from the content of a `.recipes` file.
///
/// Input:
///   Full file content, e.g.:
///   ```
///   Pepperoni =
///       MakeDough
///       -> AddBase(base_type=tomato)
///       -> AddCheese(amount=2)
///
///   Margherita =
///       MakeDough
///       -> [AddCheese(amount=2), AddBasil(leaves=3)]
///   ```
///
/// Output:
///   `Ok(Vec<Recipe>)` — one `Recipe` per blank-line-separated block,
///   in file order.
///   `Err(ParseError)` — the first error encountered.
pub fn parse_recipes(input: &str) -> Result<Vec<Recipe>, ParseError> {
    // Normalise Windows line endings, then split on blank lines.
    let normalised = input.replace("\r\n", "\n");

    normalised
        .split("\n\n")
        .map(str::trim)
        .filter(|block| !block.is_empty())
        .map(parse_recipe)
        .collect()
}

/// Expand a `Recipe`'s steps into a flat, ordered list of `ActionDef` items
/// suitable for `ExecutionContext.action_sequence`.
///
/// Input:
///   `Recipe` whose steps may include `Single`, `Parallel`, or `Repeated`.
///
/// Output:
///   `Vec<ActionDef>` — fully expanded, in execution order:
///   - `Single(a)`         → `[a]`
///   - `Parallel([a, b])`  → `[a, b]`  (sequential in the flat list)
///   - `Repeated(a, n)`    → `[a, a, … a]`  (n copies)
///
/// Example — `QuattroFormaggi`:
///   Steps:  Single(MakeDough), Single(AddBase{cream}), Repeated(AddCheese{1}, 4), Single(Bake{6}), Single(AddOliveOil)
///   Output: [MakeDough, AddBase{cream}, AddCheese{1}, AddCheese{1}, AddCheese{1}, AddCheese{1}, Bake{6}, AddOliveOil]
pub fn flatten_recipe(recipe: &Recipe) -> Vec<ActionDef> {
    let mut sequence = Vec::new();

    for step in &recipe.steps {
        match step {
            Step::Single(action) => sequence.push(action.clone()),
            Step::Parallel(actions) => sequence.extend(actions.iter().cloned()),
            Step::Repeated(action, n) => {
                for _ in 0..*n {
                    sequence.push(action.clone());
                }
            }
        }
    }

    sequence
}

// ── Internal parsing ──────────────────────────────────────────────────────────

/// Parse a single recipe block.
///
/// Accepts **both** the multi-line file format and the flat single-line wire format
/// used in `RecipeAnswer.recipe`, because both are unified by splitting on `->`:
///
/// Multi-line (from file):
///   ```
///   "Pepperoni =\n    MakeDough\n    -> AddBase(base_type=tomato)"
///   ```
/// Flat / wire (from `recipe_answer`):
///   ```
///   "Pepperoni = MakeDough -> AddBase(base_type=tomato)"
///   ```
///
/// Both produce the same `Recipe` after parsing.
/// `Recipe.source` is set to the canonical single-line DSL via `to_dsl_string()`.
fn parse_recipe(block: &str) -> Result<Recipe, ParseError> {
    let eq_pos = block.find('=').ok_or(ParseError::MissingEquals)?;

    let name = block[..eq_pos].trim().to_string();
    if name.is_empty() {
        return Err(ParseError::EmptyName);
    }

    // Split on "->" — works for both multi-line and flat wire formats.
    // Multi-line: "\n    MakeDough\n    -> AddBase(...)" → ["MakeDough", "AddBase(...)"]
    // Flat wire:  " MakeDough -> AddBase(...)"           → ["MakeDough", "AddBase(...)"]
    let step_bodies: Vec<&str> = block[eq_pos + 1..]
        .split("->")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();

    if step_bodies.is_empty() {
        return Err(ParseError::NoSteps { recipe: name.clone() });
    }

    let steps = step_bodies
        .iter()
        .enumerate()
        .map(|(idx, body)| parse_step(body, idx + 1))
        .collect::<Result<Vec<Step>, ParseError>>()?;

    let source = {
        // Temporarily build the recipe without source to generate the canonical string.
        let r = Recipe { name: name.clone(), steps: steps.clone(), source: String::new() };
        r.to_dsl_string()
    };

    Ok(Recipe { name, steps, source })
}

/// Parse one step body (after `->` has been stripped).
///
/// Input (examples):
///   - `"MakeDough"`                           → `Single`
///   - `"AddBase(base_type=tomato)"`           → `Single`
///   - `"[AddCheese(amount=2), AddBasil(leaves=3)]"` → `Parallel`
///   - `"AddCheese(amount=1)^4"`               → `Repeated`
///
/// Output: `Ok(Step)` or `Err(ParseError)`.
fn parse_step(body: &str, step_idx: usize) -> Result<Step, ParseError> {
    if body.starts_with('[') {
        parse_parallel_step(body, step_idx)
    } else if let Some(caret_pos) = find_repeat_caret(body) {
        parse_repeated_step(body, caret_pos, step_idx)
    } else {
        Ok(Step::Single(parse_action(body, step_idx)?))
    }
}

/// Find the position of `^` that introduces a repeat count.
/// The caret must be outside any parentheses and followed only by one or more digits.
///
/// Input:  `"AddCheese(amount=1)^4"` → `Some(19)`
///         `"AddOliveOil"`           → `None`
///         `"Bad^"` (no digits)      → `None`
fn find_repeat_caret(body: &str) -> Option<usize> {
    let mut depth: usize = 0;

    for (i, ch) in body.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            '^' if depth == 0 => {
                let suffix = &body[i + 1..];
                if !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()) {
                    return Some(i);
                }
            }
            _ => {}
        }
    }

    None
}

/// Parse a parallel step `"[ActionA(...), ActionB(...)]"`.
///
/// Input:  `"[AddCheese(amount=2), AddBasil(leaves=3)]"`
/// Output: `Ok(Step::Parallel(vec![
///     ActionDef { name: "AddCheese", params: {"amount": "2"} },
///     ActionDef { name: "AddBasil",  params: {"leaves": "3"} },
/// ]))`
fn parse_parallel_step(body: &str, step_idx: usize) -> Result<Step, ParseError> {
    let inner = body
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .ok_or(ParseError::UnmatchedBracket { step: step_idx })?;

    let actions = split_parallel_actions(inner)
        .into_iter()
        .filter(|t| !t.is_empty())
        .map(|token| parse_action(token.trim(), step_idx))
        .collect::<Result<Vec<ActionDef>, ParseError>>()?;

    if actions.is_empty() {
        return Err(ParseError::UnmatchedBracket { step: step_idx });
    }

    Ok(Step::Parallel(actions))
}

/// Parse a repeated step `"Action(...)^N"`.
///
/// Input:  `"AddCheese(amount=1)^4"`, `caret_pos = 19`
/// Output: `Ok(Step::Repeated(ActionDef { name: "AddCheese", params: {"amount": "1"} }, 4))`
fn parse_repeated_step(body: &str, caret_pos: usize, step_idx: usize) -> Result<Step, ParseError> {
    let action_part = body[..caret_pos].trim();
    let count_part = &body[caret_pos + 1..];

    let count: u32 = count_part
        .parse()
        .ok()
        .filter(|&n| n > 0)
        .ok_or(ParseError::InvalidRepeat { step: step_idx })?;

    let action = parse_action(action_part, step_idx)?;

    Ok(Step::Repeated(action, count))
}

/// Parse a single action `"Name"` or `"Name(key=value, key=value)"`.
///
/// Input:  `"AddBase(base_type=tomato)"`
/// Output: `Ok(ActionDef { name: "AddBase", params: {"base_type": "tomato"} })`
///
/// Input:  `"MakeDough"`
/// Output: `Ok(ActionDef { name: "MakeDough", params: {} })`
///
/// Input:  `"AddBase(bad_param)"`
/// Output: `Err(ParseError::MalformedParam { token: "bad_param" })`
fn parse_action(body: &str, step_idx: usize) -> Result<ActionDef, ParseError> {
    match body.find('(') {
        None => Ok(ActionDef {
            name: body.trim().to_string(),
            params: HashMap::new(),
        }),
        Some(paren_open) => {
            let name = body[..paren_open].trim().to_string();

            let paren_close = body
                .rfind(')')
                .ok_or(ParseError::UnmatchedBracket { step: step_idx })?;

            let params = parse_params(&body[paren_open + 1..paren_close])?;

            Ok(ActionDef { name, params })
        }
    }
}

/// Parse a comma-separated parameter list `"key=value, key=value"`.
///
/// Input:  `"base_type=tomato"`           → `Ok({"base_type": "tomato"})`
/// Input:  `"amount=2"`                   → `Ok({"amount": "2"})`
/// Input:  `"slices=12,duration=6"`       → `Ok({"slices": "12", "duration": "6"})`
/// Input:  `""`                           → `Ok({})` (no params)
/// Input:  `"bad"`                        → `Err(MalformedParam { token: "bad" })`
fn parse_params(params_str: &str) -> Result<HashMap<String, String>, ParseError> {
    let mut map = HashMap::new();

    if params_str.trim().is_empty() {
        return Ok(map);
    }

    for token in params_str.split(',') {
        let token = token.trim();
        let eq_pos = token
            .find('=')
            .ok_or_else(|| ParseError::MalformedParam { token: token.to_string() })?;

        let key = token[..eq_pos].trim().to_string();
        let value = token[eq_pos + 1..].trim().to_string();
        map.insert(key, value);
    }

    Ok(map)
}

/// Split a comma-separated list of action tokens, tracking parenthesis depth
/// so that commas inside `(...)` are not treated as action separators.
/// Used to tokenize the inside of a parallel step after stripping `[` and `]`.
///
/// Input:  `"AddCheese(amount=2), AddBasil(leaves=3)"`
/// Output: `["AddCheese(amount=2)", "AddBasil(leaves=3)"]`
///
/// Input:  `"A(x=1,y=2), B"` (comma inside params is not a separator)
/// Output: `["A(x=1,y=2)", "B"]`
fn split_parallel_actions(s: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut depth: usize = 0;

    for ch in s.chars() {
        match ch {
            '(' => {
                depth += 1;
                current.push(ch);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                current.push(ch);
            }
            ',' if depth == 0 => {
                tokens.push(current.trim().to_string());
                current = String::new();
            }
            _ => current.push(ch),
        }
    }

    let last = current.trim().to_string();
    if !last.is_empty() {
        tokens.push(last);
    }

    tokens
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_params ─────────────────────────────────────────────────────────

    #[test]
    fn params_empty() {
        // Input:  ""
        // Output: {}
        let result = parse_params("").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn params_single() {
        // Input:  "base_type=tomato"
        // Output: {"base_type": "tomato"}
        let result = parse_params("base_type=tomato").unwrap();
        assert_eq!(result["base_type"], "tomato");
    }

    #[test]
    fn params_multiple() {
        // Input:  "slices=12"
        // Output: {"slices": "12"}
        let result = parse_params("slices=12").unwrap();
        assert_eq!(result["slices"], "12");
    }

    #[test]
    fn params_malformed() {
        // Input:  "bad" (no '=')
        // Output: Err(MalformedParam)
        assert!(matches!(
            parse_params("bad"),
            Err(ParseError::MalformedParam { .. })
        ));
    }

    // ── parse_action ─────────────────────────────────────────────────────────

    #[test]
    fn action_no_params() {
        // Input:  "MakeDough"
        // Output: ActionDef { name: "MakeDough", params: {} }
        let a = parse_action("MakeDough", 1).unwrap();
        assert_eq!(a.name, "MakeDough");
        assert!(a.params.is_empty());
    }

    #[test]
    fn action_with_params() {
        // Input:  "AddBase(base_type=tomato)"
        // Output: ActionDef { name: "AddBase", params: {"base_type": "tomato"} }
        let a = parse_action("AddBase(base_type=tomato)", 1).unwrap();
        assert_eq!(a.name, "AddBase");
        assert_eq!(a.params["base_type"], "tomato");
    }

    // ── split_parallel_actions ───────────────────────────────────────────────

    #[test]
    fn split_two_actions() {
        // Input:  "AddCheese(amount=2), AddBasil(leaves=3)"
        // Output: ["AddCheese(amount=2)", "AddBasil(leaves=3)"]
        let tokens = split_parallel_actions("AddCheese(amount=2), AddBasil(leaves=3)");
        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[0], "AddCheese(amount=2)");
        assert_eq!(tokens[1], "AddBasil(leaves=3)");
    }

    #[test]
    fn split_respects_paren_depth() {
        // Comma inside params must NOT split the token.
        // Input:  "A(x=1,y=2), B"
        // Output: ["A(x=1,y=2)", "B"]
        let tokens = split_parallel_actions("A(x=1,y=2), B");
        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[0], "A(x=1,y=2)");
    }

    // ── find_repeat_caret ────────────────────────────────────────────────────

    #[test]
    fn caret_found() {
        // Input:  "AddCheese(amount=1)^4"
        // Output: Some(position of '^')
        assert!(find_repeat_caret("AddCheese(amount=1)^4").is_some());
    }

    #[test]
    fn caret_absent() {
        // Input:  "AddOliveOil"
        // Output: None
        assert!(find_repeat_caret("AddOliveOil").is_none());
    }

    #[test]
    fn caret_no_digits_is_none() {
        // Input:  "AddCheese^" (no digit suffix)
        // Output: None
        assert!(find_repeat_caret("AddCheese^").is_none());
    }

    // ── parse_step ───────────────────────────────────────────────────────────

    #[test]
    fn step_single() {
        // Input:  "Bake(duration=6)"
        // Output: Step::Single(ActionDef { name: "Bake", params: {"duration": "6"} })
        assert!(matches!(
            parse_step("Bake(duration=6)", 1).unwrap(),
            Step::Single(_)
        ));
    }

    #[test]
    fn step_parallel() {
        // Input:  "[AddCheese(amount=2), AddBasil(leaves=3)]"
        // Output: Step::Parallel(...)
        assert!(matches!(
            parse_step("[AddCheese(amount=2), AddBasil(leaves=3)]", 3).unwrap(),
            Step::Parallel(_)
        ));
    }

    #[test]
    fn step_repeated() {
        // Input:  "AddCheese(amount=1)^4"
        // Output: Step::Repeated(ActionDef { "AddCheese", {"amount":"1"} }, 4)
        let step = parse_step("AddCheese(amount=1)^4", 3).unwrap();
        assert!(matches!(step, Step::Repeated(_, 4)));
    }

    // ── parse_recipes ────────────────────────────────────────────────────────

    #[test]
    fn parse_pepperoni_multiline() {
        // Input: Pepperoni recipe in multi-line file format
        // Output: Recipe with 5 steps, all Single
        let input = "Pepperoni =\n    MakeDough\n    -> AddBase(base_type=tomato)\n    -> AddCheese(amount=2)\n    -> AddPepperoni(slices=12)\n    -> Bake(duration=6)";
        let recipes = parse_recipes(input).unwrap();
        assert_eq!(recipes.len(), 1);
        assert_eq!(recipes[0].name, "Pepperoni");
        assert_eq!(recipes[0].steps.len(), 5);
    }

    #[test]
    fn parse_pepperoni_flat_wire_format() {
        // Input: flat single-line format as sent in recipe_answer over the wire
        // Output: same Recipe as the multi-line version above — formats are equivalent
        let wire = "Pepperoni = MakeDough -> AddBase(base_type=tomato) -> AddCheese(amount=2) -> AddPepperoni(slices=12) -> Bake(duration=6)";
        let recipes = parse_recipes(wire).unwrap();
        assert_eq!(recipes[0].name, "Pepperoni");
        assert_eq!(recipes[0].steps.len(), 5);
    }

    #[test]
    fn multiline_and_flat_produce_same_source() {
        // Both input formats must yield the same canonical source string.
        let multiline = "Pepperoni =\n    MakeDough\n    -> AddBase(base_type=tomato)\n    -> Bake(duration=6)";
        let flat = "Pepperoni = MakeDough -> AddBase(base_type=tomato) -> Bake(duration=6)";
        let r_multi = &parse_recipes(multiline).unwrap()[0];
        let r_flat = &parse_recipes(flat).unwrap()[0];
        assert_eq!(r_multi.source, r_flat.source);
    }

    #[test]
    fn source_is_valid_dsl_that_roundtrips() {
        // source must itself be parseable and produce the same recipe.
        let input = "Pepperoni =\n    MakeDough\n    -> AddBase(base_type=tomato)\n    -> AddCheese(amount=2)\n    -> AddPepperoni(slices=12)\n    -> Bake(duration=6)";
        let original = &parse_recipes(input).unwrap()[0];
        // Re-parse the source string (simulating what a receiving agent does with recipe_answer)
        let reparsed = &parse_recipes(&original.source).unwrap()[0];
        assert_eq!(original.source, reparsed.source);
        assert_eq!(original.steps.len(), reparsed.steps.len());
    }

    #[test]
    fn parse_margherita_parallel() {
        // Input: Margherita recipe block with a parallel step
        // Output: Recipe with steps including one Step::Parallel
        let input = "Margherita =\n    MakeDough\n    -> AddBase(base_type=tomato)\n    -> [AddCheese(amount=2), AddBasil(leaves=3)]\n    -> Bake(duration=5)\n    -> AddOliveOil";
        let recipes = parse_recipes(input).unwrap();
        assert_eq!(recipes[0].name, "Margherita");
        assert!(matches!(recipes[0].steps[2], Step::Parallel(_)));
    }

    #[test]
    fn parse_quattro_formaggi_repeated() {
        // Input: QuattroFormaggi recipe block with a repeated step
        // Output: Recipe with one Step::Repeated(_, 4)
        let input = "QuattroFormaggi =\n    MakeDough\n    -> AddBase(base_type=cream)\n    -> AddCheese(amount=1)^4\n    -> Bake(duration=6)\n    -> AddOliveOil";
        let recipes = parse_recipes(input).unwrap();
        assert!(matches!(recipes[0].steps[2], Step::Repeated(_, 4)));
    }

    #[test]
    fn parse_multiple_recipes() {
        // Input: two recipe blocks separated by a blank line
        // Output: Vec<Recipe> with len == 2
        let input = "Pepperoni =\n    MakeDough\n    -> Bake(duration=6)\n\nMarinara =\n    MakeDough\n    -> AddOliveOil";
        let recipes = parse_recipes(input).unwrap();
        assert_eq!(recipes.len(), 2);
        assert_eq!(recipes[0].name, "Pepperoni");
        assert_eq!(recipes[1].name, "Marinara");
    }

    // ── flatten_recipe ───────────────────────────────────────────────────────

    #[test]
    fn flatten_repeated_expands() {
        // Input:  QuattroFormaggi (AddCheese^4)
        // Output: action_sequence contains 4 consecutive AddCheese entries
        let input = "QuattroFormaggi =\n    MakeDough\n    -> AddBase(base_type=cream)\n    -> AddCheese(amount=1)^4\n    -> Bake(duration=6)\n    -> AddOliveOil";
        let recipes = parse_recipes(input).unwrap();
        let seq = flatten_recipe(&recipes[0]);
        // MakeDough, AddBase, AddCheese x4, Bake, AddOliveOil = 8
        assert_eq!(seq.len(), 8);
        assert_eq!(seq[2].name, "AddCheese");
        assert_eq!(seq[3].name, "AddCheese");
        assert_eq!(seq[4].name, "AddCheese");
        assert_eq!(seq[5].name, "AddCheese");
    }

    #[test]
    fn flatten_parallel_inlines() {
        // Input:  Margherita ([AddCheese, AddBasil])
        // Output: AddCheese and AddBasil appear as consecutive entries
        let input = "Margherita =\n    MakeDough\n    -> AddBase(base_type=tomato)\n    -> [AddCheese(amount=2), AddBasil(leaves=3)]\n    -> Bake(duration=5)\n    -> AddOliveOil";
        let recipes = parse_recipes(input).unwrap();
        let seq = flatten_recipe(&recipes[0]);
        // MakeDough, AddBase, AddCheese, AddBasil, Bake, AddOliveOil = 6
        assert_eq!(seq.len(), 6);
        assert_eq!(seq[2].name, "AddCheese");
        assert_eq!(seq[3].name, "AddBasil");
    }

    #[test]
    fn flatten_full_examples_file() {
        // Input:  full examples.recipes content (all 5 recipes)
        // Output: no parse errors; each recipe flattens correctly
        let input = include_str!("../recipes/examples.recipes");
        let recipes = parse_recipes(input).unwrap();
        assert_eq!(recipes.len(), 5);

        // Pepperoni: MakeDough AddBase AddCheese AddPepperoni Bake = 5
        let pepperoni = recipes.iter().find(|r| r.name == "Pepperoni").unwrap();
        assert_eq!(flatten_recipe(pepperoni).len(), 5);

        // QuattroFormaggi: MakeDough AddBase AddCheese×4 Bake AddOliveOil = 8
        let qf = recipes.iter().find(|r| r.name == "QuattroFormaggi").unwrap();
        assert_eq!(flatten_recipe(qf).len(), 8);
    }
}
