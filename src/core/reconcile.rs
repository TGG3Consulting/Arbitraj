//! Файл: core/reconcile.rs
//! Что делает: сверка балансов (шаг 6 ТЗ) раз в ledger.reconcile_interval_s: балансы всех включённых
//! площадок через шлюз → экран «Балансы»; монеты, которые по учёту бота лежат на площадке (открытые
//! сделки не в пути), должны там быть — меньше — расхождение: новые сделки с этой площадкой останавливаются (пауза
//! площадки, решение 221) и уведомление risk.reconcile_mismatch. Плюс: мало BNB на газ у DEX (risk.low_bnb), мало USDT
//! (risk.low_usdt по порогу alert.min_usdt площадки — low_usdt.rs, решение 54), возврат USDT «отдельно».
//! Решение 112 (reconcile_confirm.rs): учёт площадки снимается до и после запроса её балансов — монета, по которой
//! сделка за это время сдвинулась, в круге не сверяется; расхождение по монете ставит паузу и уведомляет, только если
//! держится ledger.reconcile_confirm_rounds кругов подряд.
//! Решение 118 (Я-11): балансы одной площадки не получены (подтверждено теми же кругами) — пауза этой площадки
//! (venue_paused), а не системы; с решения 221 и расхождение монет — тоже пауза площадки. Я-21: «вложено по монетам»
//! на «Балансах» — по «тикер (адрес коротко)».
//! Решение 132: у DEX сверяется каждый кошелёк отдельно (reconcile_holders.rs): свои балансы, учёт сделок кошелька,
//! «мало USDT» и «мало BNB»; на «Балансах» — строка «площадка «кошелёк»».
//! Решение 221 (reconcile_pause.rs, Тигран 08.11): подтверждённая проблема сверки (расхождение монет или «балансы не
//! получены») ставит паузу только ЭТОЙ площадке, системный режим не меняется; журнал
//! core.control; пауза снимается сама после ledger.reconcile_confirm_rounds чистых кругов подряд ЭТОЙ площадки, если
//! её не ставил и не снимал человек или другой модуль.
//! Решение 230 (reconcile_wallet.rs): у DEX проблема кошелька ставит паузу только ЭТОМУ кошельку (Risk.wallet_paused),
//! а не площадке; снимается сама по чистым кругам этого кошелька. Постановка паузы площадке или кошельку — отдельное
//! короткое сообщение risk.reconcile_paused (чат «Неудачные» и «Общий»).
//! Кто вызывает: core/engine.rs (задача run).
//! От чего зависит: engine.rs, wait.rs, reconcile_pause.rs, reconcile_wallet.rs, usdt_return.rs, precheck.rs (BNB),
//! deal.rs,
//! reconcile_confirm.rs, reconcile_key.rs (ключ монеты: биржа — тикер, DEX — адрес; решение 211, ревью В2).
//! Настройки: ledger.reconcile_interval_s, ledger.reconcile_confirm_rounds, alert.min_usdt (через low_usdt.rs),
//! risk.min_bnb_balance, venue.quote_asset.

use std::collections::{HashMap, HashSet};

use rust_decimal::Decimal;
use serde_json::json;
use tokio::sync::Notify;

use crate::core::types::VenueId;
use crate::gateway::contract::VenueKind;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::engine::{Core, lk, rd};
use super::reconcile_confirm::{Streak, UNAVAILABLE, expected, stable};
use super::view::CoreEvent;
use super::wait::{Pacer, Unit};

/// Проблема круга: код события, площадка, текст; ключ «площадка/монета» — у расхождения по монете (оно ждёт
/// подтверждения кругами подряд), None — у остальных (сразу); кошелёк DEX (id) — у проблемы кошелька (решение 230:
/// пауза ставится кошельку, а не площадке), None — у биржи.
pub type Found = (
    &'static str,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
);

/// Задача сверки. Работает, пока жива система. Уведомление о каждой проблеме — один раз, пока она есть.
pub async fn run(core: Core) {
    let mut pacer = Pacer::new("ledger.reconcile_interval_s", Unit::Seconds, None);
    let quiet = Notify::new();
    let mut active: HashSet<String> = HashSet::new();
    let mut streak = Streak::default();
    let mut lift = super::reconcile_pause::Lift::default();
    // Решение 230: свой счёт чистых кругов у кошельков DEX на паузе.
    let mut lift_w = super::reconcile_pause::Lift::default();
    loop {
        pacer.pause(&core.inner.deps, &quiet).await;
        let (all, checked, wallets) = once(&core).await;
        // Решение 221: пауза площадки от сверки снимается сама после ledger.reconcile_confirm_rounds чистых кругов
        // подряд этой площадки; решение 230 — так же пауза кошелька DEX по его кругам.
        super::reconcile_pause::maybe_lift(&core, &mut lift, &checked, &all).await;
        super::reconcile_wallet::maybe_lift(&core, &mut lift_w, &wallets, &all).await;
        let keys: Vec<String> = all.iter().filter_map(|f| f.3.clone()).collect();
        let (confirmed, waiting) = streak.mark(&core, &keys);
        for (key, n, need) in waiting {
            core.inner
                .deps
                .log(
                    Level::Info,
                    "core.reconcile",
                    None,
                    format!(
                        "Расхождение {key}: круг {n} из {need} — паузы пока нет, ждём подтверждения"
                    ),
                    None,
                )
                .await;
        }
        let found: Vec<Found> = all
            .into_iter()
            .filter(|f| f.3.as_ref().is_none_or(|k| confirmed.contains(k)))
            .collect();
        // Решения 118 (Я-11) и 221: расхождение или «балансы не получены» — пауза только этой площадки, с записью в
        // журнал и автоснятием (reconcile_pause.rs); решение 230: у DEX — пауза только этого кошелька
        // (reconcile_wallet.rs).
        super::reconcile_pause::pause(&core, &found).await;
        super::reconcile_wallet::pause(&core, &found).await;
        for (code, venue, text, _, _) in &found {
            if active.insert(format!("{code}{text}")) {
                core.inner
                    .deps
                    .log(Level::Warn, "core.reconcile", venue.as_deref(), text, None)
                    .await;
                core.inner.deps.notify(code, venue.as_deref(), text).await;
            }
        }
        active.retain(|k| found.iter().any(|(c, _, t, _, _)| &format!("{c}{t}") == k));
    }
}

/// Один круг сверки. Возвращает: найденные проблемы (Found), площадки, которые сверялись в круге (решение 221: круг
/// чистый только у сверенной площадки), и кошельки DEX, которые сверялись (решение 230). Пауза — в run (после
/// подтверждения).
async fn once(core: &Core) -> (Vec<Found>, HashSet<String>, HashSet<String>) {
    let deps = &core.inner.deps;
    let venues: Vec<(VenueId, VenueKind)> = rd(&core.inner.kinds)
        .iter()
        .filter(|(v, _)| super::registry_sync::enabled(core, v))
        .cloned()
        .collect();
    let mut out = vec![];
    let mut checked = HashSet::new();
    let mut wallets = HashSet::new();
    let mut all = serde_json::Map::new();
    let mut free_usdt = HashMap::new();
    // Решение 132: у DEX — каждый кошелёк отдельно (свои балансы, учёт, «мало USDT/BNB»); подпись — «площадка «кошелёк»».
    for (v, kind, acc, label) in super::reconcile_holders::list(core, venues) {
        checked.insert(v.0.clone());
        // Решение 230: проблема кошелька DEX несёт его id — пауза ставится кошельку.
        let w = acc.as_ref().map(|a| a.0.clone());
        if let Some(id) = &w {
            wallets.insert(id.clone());
        }
        let at = At {
            venue: Some(&v.0),
            coin: None,
        };
        // Учёт площадки до и после запроса балансов (решение 112): сверяются только монеты, по которым сделки за это
        // время не сдвинулись.
        let before = expected(core, &v, acc.as_ref());
        let g = deps.gw.get_as(&v, acc.as_ref()).await;
        let b = match &g {
            Ok(x) => x.balances().await,
            Err(e) => Err(e.clone()),
        };
        let held = stable(&before, &expected(core, &v, acc.as_ref()));
        let b = match b {
            Ok(b) => b,
            Err(e) => {
                out.push((
                    "risk.reconcile_mismatch",
                    Some(v.0.clone()),
                    format!("{label}: балансы не получены — {}", e.message),
                    Some(format!("{label}{UNAVAILABLE}")),
                    w.clone(),
                ));
                continue;
            }
        };
        all.insert(label.clone(), json!(b));
        // Решение 140: у кошелька DEX нет строки USDT — «нет данных», не 0: ни алерта, ни перегона «отдельно».
        if let Ok(q) = deps.settings.text("venue.quote_asset", at)
            && let Some(f) = super::low_usdt::free_of(kind, &b, &q)
        {
            free_usdt.insert((v.clone(), acc.clone()), f);
            // Решение 54: свой порог алерта по площадке, один раз на пересечение (low_usdt.rs); у DEX — по кошельку.
            super::low_usdt::check(core, &v, acc.as_ref(), &label, f, &q).await;
        }
        // Мало BNB на газ — по уже полученным балансам (решение 113: раньше bnb_ok запрашивал балансы кошелька второй
        // раз за круг). Резерв не задан — та же причина, что у bnb_ok.
        if kind == VenueKind::Dex {
            let have: Decimal = b
                .iter()
                .filter(|x| x.asset == super::convert::GAS_TICKER)
                .map(|x| x.free)
                .sum();
            let r = deps
                .settings
                .decimal("risk.min_bnb_balance", At::default())
                .map_err(|e| e.message)
                .and_then(|min| super::precheck::bnb_enough(&label, have, min, Decimal::ZERO));
            if let Err(e) = r {
                out.push(("risk.low_bnb", Some(v.0.clone()), e, None, w.clone()));
            }
        }
        let Ok(gw) = &g else { continue };
        for (key, need) in &held {
            let ev = &v;
            // Решение 211 (ревью В2): у кошелька DEX — остаток по адресу (reconcile_key.rs), у биржи — строки по тикеру.
            let ticker = &super::reconcile_key::name(core, key);
            // Ключ уведомления — в прежнем формате «площадка/тикер», адрес только в тексте (второе ревью 211, п.9).
            let short = super::reconcile_key::short(core, key);
            let total = match super::reconcile_key::total(kind, gw.as_ref(), &b, key).await {
                Ok(t) => t,
                Err(e) => {
                    let text = format!("{label}: {ticker}: {e}");
                    let k = Some(format!("{label}/{short}"));
                    let found = (
                        "risk.reconcile_mismatch",
                        Some(ev.0.clone()),
                        text,
                        k,
                        w.clone(),
                    );
                    out.push(found);
                    continue;
                }
            };
            // BNB на DEX (решение 62): нативный + WBNB (адаптер отдаёт WBNB в locked строки BNB) минус резерв на газ —
            // резерв монетами сделки не считается; резерв не задан — сверить нельзя, это и есть причина.
            let native = super::reconcile_key::is_native_key(core, key);
            let have = match super::native::spendable(core, ev, native, ticker, total) {
                Ok(h) => h,
                Err(e) => {
                    out.push((
                        "risk.reconcile_mismatch",
                        Some(ev.0.clone()),
                        e,
                        None,
                        w.clone(),
                    ));
                    continue;
                }
            };
            if have < *need {
                out.push((
                    "risk.reconcile_mismatch",
                    Some(ev.0.clone()),
                    format!("{label}: {ticker} по учёту {need}, на площадке {have}"),
                    Some(format!("{label}/{short}")),
                    w.clone(),
                ));
            }
        }
    }
    let ex = lk(&core.inner.exposure).clone();
    // Решение 118 (Я-21): вложено по монетам — ключ «тикер (адрес коротко)»: монеты с одним тикером не склеиваются.
    let per_coin: HashMap<String, Decimal> = ex
        .per_coin
        .iter()
        .map(|(a, v)| (super::coin_key::label(a), *v))
        .collect();
    core.inner.live.send(CoreEvent::Balances(
        json!({ "venues": all, "in_transit_usd": ex.in_transit, "per_coin_usd": per_coin }),
    ));
    super::usdt_return::separate(core, &free_usdt).await;
    (out, checked, wallets)
}
