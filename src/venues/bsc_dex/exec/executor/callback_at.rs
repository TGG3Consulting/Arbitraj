//! Файл: venues/bsc_dex/exec/executor/callback_at.rs
//! Что делает: задача t122 (п. 1) — повторная проверка пути исполнения пула после сверки исполнителя по версии,
//! которую ВЕРНУЛА сверка onchain::ensure (код в сети), а не по записи версии у кошелька. Первичный отбор пулов
//! по-прежнему идёт по записи у кошелька (pools/exec_kind.rs via_executor → exec_callback::missing; без записи —
//! via_executor = нет, и до ensure дело не доходит: check/probe_exec.rs part, quote/graph_executor.rs all_via,
//! exec/executor/mod.rs takes). Исход здесь меняется, только когда запись говорит «можно», а сверка — «нельзя»
//! (запись отстала), или запись не обновилась при сбое onchain.rs remember. Правила — те же, что exec_callback missing:
//! вид шага с версии выше сверенной (exec_callback::since / step_absent: решение 284 — с v2, решение 390 — с v3) —
//! «переразверните исполнитель»;
//! обратный вызов пула dex.v3_swap_callback (ревью 281, задача t114) — по артефакту этой версии (exec_callback::check).
//! Кто вызывает: exec/executor/trade.rs (сделка через исполнитель), check/probe_exec.rs (проба налога через исполнитель).
//! От чего зависит: pools/exec_callback.rs (SETTING, step_absent, check), pools/families.rs (Family), code.rs (Version), ctx.rs.
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
    if let Some(why) = f
        .executor_kind
        .and_then(|k| exec_callback::step_absent(k, v))
    {
        return Some(why);
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
