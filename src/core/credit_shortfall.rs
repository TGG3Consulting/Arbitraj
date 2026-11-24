//! Файл: core/credit_shortfall.rs
//! Что делает: страховка от неизмеренного налога перевода (решение 118, Я-3): перевод зачислен, а дошло меньше
//! ожидаемого (вывод минус комиссия вывода и известный налог — credit_expect::arrival, как в расчёте) больше чем на
//! transfer.credit_shortfall_pct — монета уходит в ручной чёрный список (coin.blocked = да, coin.blocked_note —
//! «зачислено меньше на N% — похоже на налог перевода»), журнал и уведомление coin.credit_shortfall. Больше эта монета
//! не торгуется, пока человек не снимет отметку на экране «Монеты». USDT (перегон) не проверяется — его блок
//! остановил бы всю торговлю, а налога у него нет.
//! Решение 253 (д): каждое зачисление пишет факты перевода (фактическая комиссия, недостача) — withdraw_seen.rs,
//! независимо от порога transfer.credit_shortfall_pct.
//! Решение 355: отметка — сразу в памяти настроек (core/coin_mark.rs), в базу — фоном; журнал и уведомление — очереди.
//! Кто вызывает: core/transfers.rs (после зачисления), core/transfer_chain.rs (зачисление по блокчейну — после
//! фактической комиссии из статуса вывода, решение 253).
//! От чего зависит: credit_expect.rs, engine.rs, settings (запись ручной отметки), deps.rs (журнал, уведомления).
//! Настройки: transfer.credit_shortfall_pct (вся система, необязательная; не задана — без авто-блока);
//! пишет coin.blocked и coin.blocked_note (по монете — тикер, как ручной список).

use rust_decimal::Decimal;

use crate::core::types::{Asset, Cost, CostKind, Direction, VenueId};
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::engine::Core;

/// Кто меняет настройку (журнал изменений настроек).
const AUTHOR: &str = "ядро: зачислено меньше ожидаемого (решение 118)";

#[path = "credit_units.rs"]
mod units;
pub use units::CreditProof;

/// Сверяет зачисленное с ожидаемым и при недостаче больше допуска блокирует монету. Принимает: сделку, откуда,
/// куда, монету, сеть отправителя, сумму вывода, зачислено, комиссию вывода из ответа площадки-отправителя (решение
/// 247, п. Ж: Transfer.fee ответа на вывод / статуса; в этой монете — ожидание = (вывод − она) × (1 − известный налог),
/// вместо комиссии справочника; None или не в монете — как раньше, credit_expect::arrival).
/// Ошибок нет: ожидание не посчитать — проверки нет.
#[allow(clippy::too_many_arguments)]
pub async fn check(
    core: &Core,
    deal: &str,
    from: &VenueId,
    to: &VenueId,
    asset: &Asset,
    net: &str,
    sent: Decimal,
    credited: Decimal,
    fee: Option<&Cost>,
    proof: Option<CreditProof<'_>>,
    pinned: Option<super::settlement_unit::UnitRatio>,
) {
    // Решение 430: USDT сети монеты.
    if super::registry_chains::usdt_of(core, asset.chain.as_ref())
        .ok()
        .as_ref()
        == asset.contract.as_ref()
    {
        return;
    }
    let current = super::credit_expect::units(core, from, to, asset).ok();
    if current.is_none() {
        let core = core.clone();
        let dir = Direction {
            asset: asset.clone(),
            buy_on: from.clone(),
            sell_on: to.clone(),
        };
        let deal = deal.to_owned();
        let net = net.to_owned();
        let fee = fee.cloned();
        let proof = proof.map(|p| {
            (
                p.accepted_id.to_owned(),
                p.sender.clone(),
                p.receiver.clone(),
                p.wallet.clone(),
            )
        });
        // Восстановление справочника для учёта не задерживает продажу уже зачисленного актива.
        tokio::spawn(async move {
            let current = super::settlement_unit::confirmed(&core, &dir).await.ok();
            let proof = proof.as_ref().map(|(id, s, r, w)| CreditProof {
                accepted_id: id,
                sender: s,
                receiver: r,
                wallet: w,
            });
            record(
                &core,
                &deal,
                &dir.buy_on,
                &dir.sell_on,
                &dir.asset,
                &net,
                sent,
                credited,
                fee.as_ref(),
                proof,
                pinned,
                current,
            )
            .await;
        });
        return;
    }
    record(
        core, deal, from, to, asset, net, sent, credited, fee, proof, pinned, current,
    )
    .await;
}

#[allow(clippy::too_many_arguments)]
async fn record(
    core: &Core,
    deal: &str,
    from: &VenueId,
    to: &VenueId,
    asset: &Asset,
    net: &str,
    sent: Decimal,
    credited: Decimal,
    fee: Option<&Cost>,
    proof: Option<CreditProof<'_>>,
    units: Option<super::settlement_unit::UnitRatio>,
    current: Option<super::settlement_unit::UnitRatio>,
) {
    let deps = &core.inner.deps;
    // Решение 247, п. Ж: комиссия вывода — факт из ответа площадки, если она в этой монете (имя площадки → ядра).
    let fact_fee = fee
        .filter(|c| {
            c.kind == CostKind::WithdrawFee
                && c.paid_in.is_none()
                && !c.in_price
                && super::venue_ticker::name_back(core, asset, from, &c.asset) == asset.ticker
        })
        .map(|c| c.amount);
    let expected = match fact_fee {
        Some(f) => match super::transfer_tax::pct(core, to, asset) {
            Ok(tax) => (sent - f) * (Decimal::ONE - tax / Decimal::ONE_HUNDRED),
            Err(_) => return,
        },
        None => match super::credit_expect::sender_amount(core, from, to, asset, net, sent).await {
            Ok(e) => e,
            Err(_) => return,
        },
    };
    // Решение 253 (д): факты перевода — фактическая комиссия и недостача — для затрат следующих сделок по монете с
    // этой площадки (withdraw_seen.rs; в расчёт — при transfer.seen_cost). Пишутся всегда, порог ниже — отдельно.
    // Дополнение (Тигран 10.11): сырые числа (отправлено, зачислено, комиссия по факту) — разложение на комиссию, налог и
    // прочее делает расчёт против своей текущей модели (без двойного счёта при последующем замере налога).
    let mut seen = super::withdraw_seen::Seen {
        asset: asset.clone(),
        from: from.clone(),
        sent,
        credited,
        fee: fact_fee,
    };
    let mut receipt = None;
    let mut in_sender_units = credited;
    if let Some(r) = self::units::stable(units, current)
        && let Some(hash) = proof
            .as_ref()
            .and_then(|p| p.matched_tx(core, from, to, asset))
        && let Ok(in_a) = r.to_buy(credited)
    {
        in_sender_units = in_a;
        receipt = Some(super::withdraw_seen::Receipt {
            tx: hash.to_owned(),
            raw: credited,
            requested: sent,
            venue_units: Some(r),
        });
    }
    if expected != in_sender_units
        && let (Some(p), Some(f)) = (proof.as_ref(), fact_fee)
        && p.sender.amount - f != in_sender_units
        && let Some((hash, ui, gross)) = p.in_sender_units(core, to, net, &seen, expected, f).await
    {
        receipt = Some(super::withdraw_seen::Receipt {
            tx: hash.to_owned(),
            raw: credited,
            requested: sent,
            venue_units: None,
        });
        seen.sent = gross;
        in_sender_units = ui;
        deps.log(Level::Info, "core.credit_shortfall", Some(&to.0),
            format!("{deal}: вывод {hash} подтверждён в двух единицах: зачислено {credited} в контракте, {ui} в единицах отправителя после комиссии {f}"), None).await;
    }
    if receipt.is_none() && !self::units::same_identity(units, current) {
        deps.log(Level::Warn, "core.credit_shortfall", Some(&to.0),
            format!("{deal}: по хешу не подтверждён пересчёт {sent} → {credited} между {} и {}; фактическое зачисление сохранено в единицах получателя", from.0, to.0), None).await;
        return;
    }
    seen.credited = in_sender_units;
    let credited = in_sender_units;
    super::withdraw_seen::record_for(core, to, seen, receipt).await;
    let Ok(max) = deps
        .settings
        .decimal("transfer.credit_shortfall_pct", At::default())
    else {
        return;
    };
    let Some(pct) = shortfall_pct(expected, credited) else {
        return;
    };
    if pct <= max {
        return;
    }
    let pct = pct.round_dp(4);
    // Решение 247, п. Ж: текст — по фактам: комиссия из ответа площадки известна — недостача сверх неё (налог
    // перевода); нет — комиссия по справочнику сетей, недостача — комиссия вывода или налог перевода.
    let note = match fact_fee {
        Some(f) => format!(
            "зачислено меньше на {pct}% сверх комиссии вывода {f} {} из ответа {} — налог перевода ({deal}: {} → {}, отправлено {sent}, ожидалось {expected}, пришло {credited})",
            asset.ticker, from.0, from.0, to.0
        ),
        None => format!(
            "зачислено меньше на {pct}% — комиссия вывода или налог перевода: {} комиссию вывода в ответе не сообщила, ожидание — по справочнику сетей ({deal}: {} → {}, отправлено {sent}, ожидалось {expected}, пришло {credited})",
            from.0, from.0, to.0
        ),
    };
    // Решение 355 (ревью t158 Б1): отметка действует сразу — в памяти настроек до следующего шага сделки (продажа не
    // ждёт основную базу, новая сделка по монете уже видит отметку); в базу — своей задачей хранилища.
    let text = match super::coin_mark::block_now(&deps.settings, asset, &note, AUTHOR) {
        Ok(()) => format!("{}: в ручном чёрном списке — {note}", asset.ticker),
        Err(e) => format!(
            "{}: {note}; в чёрный список не внесена ({e}) — внесите вручную",
            asset.ticker
        ),
    };
    deps.log(
        Level::Warn,
        "core.credit_shortfall",
        Some(&to.0),
        &text,
        None,
    )
    .await;
    // Решение 408 (исключение из решения 157): «Убрать из чёрного списка» на тревоге (e:ub → «Да/Нет» → Unblacklist).
    let coin = super::coin_key::scope_id(asset);
    let acts = vec![crate::notify::acts::e::unblacklist(&coin, &asset.ticker)];
    deps.notify_acts("coin.credit_shortfall", Some(&to.0), &text, acts)
        .await;
}

/// Недостача зачисления, % от ожидаемого. None — недостачи нет или ожидание не больше нуля. Ошибок нет.
pub fn shortfall_pct(expected: Decimal, credited: Decimal) -> Option<Decimal> {
    (expected > Decimal::ZERO && credited < expected)
        .then(|| (expected - credited) / expected * Decimal::ONE_HUNDRED)
}
