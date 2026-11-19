//! Файл: core/remain_alert.rs
//! Что делает: решение 405 (вынесено из remain_fact.rs, ревью t198: файл > 200 строк) — алерт «Своих монет на счёте не
//! видно»: на счёте что-то есть, но по учёту всё — монеты других сделок; сделка не закрывается как фантом и ничего не
//! продаёт, решает человек (manual.rs raise_alert_with, кнопки «Ручного управления»).
//! Кто вызывает: core/remain_fact.rs (apply_with).
//! От чего зависит: remain_others.rs, deal_words.rs, manual.rs, deps.rs (журнал core.remain_fact).
//! Настройки: нет.

use rust_decimal::Decimal;

use crate::storage::journal::Level;

use super::deal::Deal;
use super::engine::Core;

/// Решение 405: на счёте что-то есть, но по учёту всё — монеты других сделок: сделка не закрывается как фантом (чей
/// фантом — по факту не видно) и ничего не продаёт — алерт человеку (manual.rs raise_alert, кнопки «Ручного
/// управления»). Принимает: ядро, сделку, факт счёта, другие сделки. Возвращает: false (дальше автоматика не ведёт).
/// Ошибок нет.
pub async fn all_foreign(
    core: &Core,
    deal: &Deal,
    fact: Decimal,
    others: &super::remain_others::Others,
) -> bool {
    // Требование Тиграна 18.11: что произошло, какая сделка, что хотели, сколько по учёту / на счёте / у других, что
    // делать человеку.
    let t = &deal.direction.asset.ticker;
    let at = super::deal_words::place(core, &deal.at, deal.dex_account.as_ref());
    // Решение 413 (восьмое ревью t198): остаток «не присвоено» решён в 0 (количество 0) — свой текст, без «должна
    // продать 0».
    let want = if deal.qty > Decimal::ZERO {
        format!("должна продать {}", super::deal_words::coins(deal.qty, t))
    } else {
        "по остатку зачисления не получила ничего".to_string()
    };
    let mut text = format!(
        "Своих монет на счёте не видно: {} {want}, на {at} сейчас {}, но по учёту {} из них принадлежат другим \
         сделкам: {}",
        super::deal_words::deal(core, deal),
        super::deal_words::coins(fact, t),
        super::deal_words::coins(others.qty, t),
        others.words.join("; ")
    );
    text.push_str(&format!(
        ". Для этой сделки на счёте не остаётся ничего (не хватает {}). Бот ничего не продаёт и сделку не закрывает, \
         чтобы не продать чужие монеты. Что сделать: проверьте баланс {t} на {at}; если монеты этой сделки есть — \
         «Продать сейчас» по сделке {} на экране «Ручное управление», если их нет — «Продано вручную» (или закройте \
         сделку с завышенным учётом из списка выше) (решение 405)",
        super::deal_words::coins(deal.qty, t),
        deal.id
    ));
    core.inner
        .deps
        .log(
            Level::Warn,
            "core.remain_fact",
            Some(&deal.at.0),
            &text,
            None,
        )
        .await;
    // В куче: путь застрявшей сделки чувствителен к стеку (решение 389).
    // Решение 408: сделки-держатели — кнопки «Открыть сделку <ID>» у записи в Telegram (notify/mirror_view.rs).
    let mut holders: Vec<String> = others.deals.iter().map(|(id, _)| id.clone()).collect();
    holders.extend(others.unclear.iter().cloned());
    holders.sort();
    holders.dedup();
    Box::pin(super::manual::raise_alert_with(
        core,
        deal.clone(),
        None,
        text,
        None,
        holders,
    ))
    .await;
    false
}
