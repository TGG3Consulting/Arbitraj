//! Файл: venues/bsc_dex/exec/executor/callback_at.rs
//! Что делает: задача t122 (п. 1) — повторная проверка пути исполнения пула после сверки исполнителя по версии,
//! которую ВЕРНУЛА сверка onchain::ensure (код в сети), а не по записи версии у кошелька: запись может не сохраниться
//! (onchain.rs remember — сбой записи только в журнал) или отстать. Правила — те же, что pools/exec_callback.rs missing:
//! вид шага только у v2 (exec_callback::v2_only, решение 284) у исполнителя v1 — «переразверните исполнитель»;
//! обратный вызов пула dex.v3_swap_callback (ревью 281, задача t114) — по артефакту этой версии (exec_callback::check).
//! Кто вызывает: exec/executor/trade.rs (сделка через исполнитель), check/probe_exec.rs (проба налога через исполнитель).
//! От чего зависит: pools/exec_callback.rs (SETTING, v2_only, check), pools/families.rs (Family), code.rs (Version), ctx.rs.
//! Настройки: dex.v3_swap_callback [DEX] (необязательная).

use std::collections::HashMap;
use std::sync::Mutex;

use super::super::super::ctx::Ctx;
use super::super::super::pools::exec_callback;
use super::super::super::pools::families::Family;
use super::code::Version;

/// Итог поиска селектора по ключу (версия, сигнатура): артефакты contracts/out/ неизменны до перезапуска — хеш и
/// разбор кода не повторяются на каждой сделке (как память SEEN в pools/exec_callback.rs).
static SEEN: Mutex<Option<Seen>> = Mutex::new(None);

/// Память итогов: ключ (версия, сигнатура) → None — вызов есть, Some — причина.
type Seen = HashMap<(Version, String), Option<String>>;

/// Нет ли у исполнителя данной версии пути для пулов этой DEX. Принимает: окружение, вид DEX, версию из
/// onchain::ensure. Возвращает: None — вид шага и обратный вызов (если dex.v3_swap_callback задана) у этой версии есть;
/// Some — причина (текст как у pools/exec_callback.rs). Ошибок нет.
pub fn missing_at(ctx: &Ctx, f: &Family, v: Version) -> Option<String> {
    if let Some(k) = f.executor_kind.filter(|k| exec_callback::v2_only(*k))
        && v == Version::V1
    {
        return Some(format!(
            "исполнитель v1 не знает шаг исполнителя {} (вид шага есть только в v2) — переразверните исполнитель \
             кошелька (экран «Маппинг кошельков»)",
            k.name()
        ));
    }
    let sig = ctx
        .dex_text(exec_callback::SETTING, f.id)
        .ok()?
        .trim()
        .to_string();
    if sig.is_empty() {
        return None;
    }
    let mut g = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    g.get_or_insert_with(HashMap::new)
        .entry((v, sig.clone()))
        .or_insert_with(|| exec_callback::check(v, &sig))
        .clone()
}
