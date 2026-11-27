use super::{err, E, R};
use mail_core::model::{Rule, RuleActivity, RuleRun, RuleTest};

// ---------- rules ----------

#[tauri::command(async)]
pub fn rules(engine: E) -> R<Vec<Rule>> {
    engine.rules().map_err(err)
}

/// Saves a rule from the Rules settings: the user's own, so it's live at once.
#[tauri::command(async)]
pub fn save_rule(engine: E, rule: Rule) -> R<Rule> {
    engine.save_rule(rule, false).map_err(err)
}

#[tauri::command(async)]
pub fn delete_rule(engine: E, id: i64) -> R<()> {
    engine.delete_rule(id).map_err(err)
}

#[tauri::command(async)]
pub fn reorder_rules(engine: E, ids: Vec<i64>) -> R<()> {
    engine.reorder_rules(&ids).map_err(err)
}

#[tauri::command(async)]
pub fn set_rule_enabled(engine: E, id: i64, enabled: bool) -> R<()> {
    engine.set_rule_enabled(id, enabled).map_err(err)
}

#[tauri::command(async)]
pub fn check_rule(engine: E, rule: Rule) -> R<()> {
    engine.check_rule(&rule).map_err(err)
}

#[tauri::command(async)]
pub fn test_rule(engine: E, rule: Rule, folder_id: Option<i64>) -> R<RuleTest> {
    engine.test_rule(&rule, folder_id).map_err(err)
}

#[tauri::command]
pub async fn run_rule(engine: E<'_>, id: i64, folder_id: Option<i64>, include_sending: bool) -> R<RuleRun> {
    engine.run_rule(id, folder_id, include_sending).await.map_err(err)
}

#[tauri::command(async)]
pub fn rule_facts(engine: E, ids: Vec<i64>) -> R<Vec<mail_core::rules::Facts>> {
    engine.rule_facts(&ids).map_err(err)
}

#[tauri::command(async)]
pub fn rule_activity(engine: E, limit: usize) -> R<Vec<RuleActivity>> {
    engine.rule_activity(limit).map_err(err)
}

#[tauri::command(async)]
pub fn undo_rule_activity(engine: E, id: i64) -> R<()> {
    engine.undo_rule_activity(id).map_err(err)
}
