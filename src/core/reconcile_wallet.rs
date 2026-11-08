//! Файл: core/reconcile_wallet.rs
//! Что делает: пауза КОШЕЛЬКА DEX от сверки балансов и её автоснятие (решение 230, Тигран 08.11.2026). У DEX сверка
//! идёт по каждому кошельку отдельно (решение 132, reconcile_holders.rs); до решения 230 подтверждённая проблема
//! одного кошелька ставила паузу всей площадке DEX (решение 221) — останавливались все кошельки. Теперь:
//! (1) подтверждённая проблема кошелька (монет меньше, чем по учёту кошелька, или его балансы не получены — после
//!     ledger.reconcile_confirm_rounds кругов, как у площадки) ставит паузу только ЭТОМУ кошельку (Risk.wallet_paused,
//!     wallet_pause.rs), если он ещё не на паузе: новые сделки этим кошельком не начинаются, остальные кошельки
//!     торгуют; журнал core.control с именем кошелька; короткое сообщение risk.reconcile_paused — чат «Неудачные»
//!     (роль Failures, notify/routes.rs) и «Общий» по настройкам события;
//! (2) если по ЭТОМУ кошельку ledger.reconcile_confirm_rounds кругов подряд чистые (кошелёк сверялся в круге и нет ни
//!     одной его проблемы risk.reconcile_mismatch — ни подтверждённой, ни ждущей кругов) — пауза снимается сама,
//!     журнал core.control и risk.reconcile_resumed с именем кошелька (как 221б у площадки);
//! (3) «Снять паузу» человеком — Core::resume_wallet (wallet_pause.rs): пауза снята, счёт кругов забывается.
//! Здесь же — текст короткого сообщения о паузе (paused_text), его берёт и пауза площадки (reconcile_pause.rs).
//! Кто вызывает: core/reconcile.rs (run), reconcile_pause.rs (paused_text), тесты.
//! От чего зависит: engine.rs, risk.rs, wallet_pause.rs, reconcile_pause.rs (Lift, need, MISMATCH, PAUSED),
//! reconcile.rs (Found), settings/accounts (имя кошелька), notify/human.rs (значок PAUSE), notify/routes.rs (Tag).
//! Настройки: ledger.reconcile_confirm_rounds (через reconcile_pause::need).

use std::collections::{BTreeMap, HashSet};

use crate::notify::human::PAUSE;
use crate::notify::routes::Tag;
use crate::storage::journal::Level;

use super::engine::{Core, lk};
use super::reconcile::Found;
use super::reconcile_pause::{Lift, MISMATCH, PAUSED, need};
use super::wallet_pause::WalletPause;

/// Подтверждённые проблемы сверки по кошелькам DEX: id кошелька → (площадка DEX, тексты проблем). Принимает:
/// подтверждённые проблемы круга. Ошибок нет.
pub fn mismatch_by_wallet(found: &[Found]) -> BTreeMap<String, (String, Vec<String>)> {
    let mut out: BTreeMap<String, (String, Vec<String>)> = BTreeMap::new();
    for f in found.iter().filter(|f| f.0 == MISMATCH) {
        if let Some(w) = &f.4 {
            let e = out.entry(w.clone()).or_default();
            e.0 = f.1.clone().unwrap_or_default();
            e.1.push(f.2.clone());
        }
    }
    out
}

/// Круг кошелька чистый: кошелёк сверялся в этом круге и среди всех проблем круга нет ни одной его проблемы
/// risk.reconcile_mismatch. Принимает: id кошелька, сверенные кошельки, все проблемы круга. Ошибок нет.
pub fn is_clean(wallet: &str, checked: &HashSet<String>, all: &[Found]) -> bool {
    checked.contains(wallet)
        && !all
            .iter()
            .any(|f| f.0 == MISMATCH && f.4.as_deref() == Some(wallet))
}

/// Короткое сообщение о паузе от сверки: «⏸ Пауза <кто>: расхождение сверки — <монета> по учёту X, на площадке Y.
/// Снимется сама, когда балансы сойдутся». Подпись строки сверки («mexc: », «bsc_dex «Основной»: ») из текстов
/// убирается — «кто» уже в начале. Принимает: кто (площадка или «кошелёк «имя»»), подпись строки сверки, тексты
/// проблем. Ошибок нет.
pub fn paused_text(who: &str, label: &str, texts: &[&str]) -> String {
    let prefix = format!("{label}: ");
    let what: Vec<&str> = texts
        .iter()
        .map(|t| t.strip_prefix(prefix.as_str()).unwrap_or(t))
        .collect();
    format!(
        "{PAUSE} Пауза {who}: расхождение сверки — {}. Снимется сама, когда балансы сойдутся",
        what.join("; ")
    )
}

/// Ставит паузу кошельку с подтверждённой проблемой сверки, если он ещё не на паузе; журнал core.control, короткое
/// сообщение risk.reconcile_paused. Принимает: ядро, подтверждённые проблемы круга. Ошибок нет.
pub async fn pause(core: &Core, found: &[Found]) {
    let n = need(core);
    for (id, (venue, texts)) in mismatch_by_wallet(found) {
        let name = core
            .inner
            .deps
            .settings
            .accounts()
            .wallet(&id)
            .map_or_else(|| id.clone(), |w| w.name);
        let p = WalletPause {
            name,
            venue: venue.clone(),
            why: format!("сверка балансов: {}", texts.join("; ")),
        };
        let fresh = {
            let mut r = lk(&core.inner.risk);
            // Уже на паузе — прежняя запись остаётся (причина первой постановки).
            if r.wallet_paused.contains_key(&id) {
                false
            } else {
                r.wallet_paused.insert(id.clone(), p.clone());
                true
            }
        };
        if !fresh {
            continue;
        }
        super::risk::publish(core);
        let text = format!(
            "Пауза: {} ({venue}, {id}) — расхождение при сверке балансов: {}. Новые сделки этим кошельком не \
             начинаются, остальные кошельки торгуют. Снимется сама, когда по кошельку расхождений не будет {n} кругов \
             сверки подряд (решение 230)",
            p.who(),
            texts.join("; ")
        );
        let deps = &core.inner.deps;
        deps.log(Level::Info, "core.control", Some(&venue), &text, None)
            .await;
        // Подпись строки сверки кошелька — как в reconcile_holders.rs: «площадка «имя»».
        let label = format!("{venue} «{}»", p.name);
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        let short = paused_text(&p.who(), &label, &refs);
        deps.notify_tagged(PAUSED, Some(&venue), &short, Tag::Plain)
            .await;
    }
}

/// Круг сверки для автоснятия пауз кошельков: чистый круг кошелька считается; набралось кругов подряд — пауза
/// снимается, журнал core.control и risk.reconcile_resumed. Принимает: ядро, счётчик, сверенные кошельки, все проблемы
/// круга. Ошибок нет.
pub async fn maybe_lift(core: &Core, lift: &mut Lift, checked: &HashSet<String>, all: &[Found]) {
    let marked: HashSet<String> = lk(&core.inner.risk).wallet_paused.keys().cloned().collect();
    lift.retain(&marked);
    let n = need(core);
    for id in marked {
        if !lift.round(&id, true, is_clean(&id, checked, all), n) {
            continue;
        }
        let Some(p) = lk(&core.inner.risk).wallet_paused.remove(&id) else {
            continue;
        };
        super::risk::publish(core);
        let text = format!(
            "Пауза: {} ({}, {id}) от сверки балансов снята сама: расхождений нет {n} кругов сверки подряд \
             (решение 230)",
            p.who(),
            p.venue
        );
        let deps = &core.inner.deps;
        deps.log(Level::Info, "core.control", Some(&p.venue), &text, None)
            .await;
        deps.notify("risk.reconcile_resumed", Some(&p.venue), &text)
            .await;
    }
}
