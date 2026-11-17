//! Файл: venues/gate/deposit_info.rs
//! Что делает: фоновое обновление сведений депозита Gate по монетам подписки (решение 143). Минимум депозита Gate
//! отдаёт только в ответе адреса депозита (min_deposit_amount), а не в справочниках монет. По монетам подписки адрес
//! депозита в сети BSC запрашивается фоном с паузой между запросами; минимум и подтверждения сети попадают в сведения
//! монет и сразу — в собранный справочник сетей (wallet.rs note_deposit, решение 387): ядро видит их со следующим
//! опросом справочника. Годный адрес заодно попадает в кэш адресов (deposit_addr.rs).
//! Решение 387 (живой факт 16–17.11: после перезапуска «→ gate» 14 минут без минимума, круг ~1070 монет по одной при
//! 2 запросах/с, сбой у 728 — «deposit disabled»):
//! - что и в каком порядке спрашивать — deposit_plan.rs: монеты с закрытым по справочнику вводом в сети BSC (и без
//!   сети BSC) не спрашиваются; сначала монеты с неизвестным минимумом. Монеты, нужные расчёту сейчас, ядро
//!   запрашивает само, не дожидаясь круга (core/deposit_min_ask.rs → deposit_min.rs ensure);
//! - повтор после сбоя временной причины (секреты закрыты, связь, лимит — deposit_plan.rs transient): только монеты
//!   со сбоем, сразу как справочник сетей Gate снова получен удачно (после разблокировки секретов ядро перечитывает
//!   справочники — Core::secrets_changed) или по сроку полного круга; секреты закрыты — круг прекращается сразу
//!   (остальные запросы упали бы так же), монеты — в повтор;
//! - пауза venue.deposit_info_refresh_s между полными кругами прерывается сменой настроек круга (venue.deposit_info_*,
//!   venue.bsc_network, venue.deposit_min_absent_ok — вся система или gate): сразу новый полный круг.
//!
//! Итог круга — одна запись в журнал (сколько получено, у каких монет сбой — первые из списка, сколько не спрашивались).
//! Кто вызывает: subs.rs (запуск при новом списке монет подписки, как обновление комиссий — решение 131).
//! От чего зависит: deposit_addr.rs, deposit_plan.rs, deposit_min.rs, wallet.rs (Networks), ctx.rs, settings/wait.rs,
//! settings/live.rs.
//! Настройки: venue.deposit_info_requests_per_sec [gate], venue.deposit_info_refresh_s [gate], venue.bsc_network [gate].
//! Не заданы — круг ждёт настройку (запись в журнал) и продолжает сам, когда их зададут.
//!
//! Официальная документация https://www.gate.com/docs/developers/apiv4/en/ : «Generate currency deposit address»
//! GET /wallet/deposit_address?currency= (min_deposit_amount, multichain_addresses[].min_confirms); лимит частоты —
//! таблица «Frequency limit rule», строка Wallet: «The Others: 200r/10s per endpoint», считается по UID.

use std::collections::HashSet;
use std::time::Duration;

use tokio::time::Instant;

use crate::core::types::Asset;
use crate::settings::live::Watch;
use crate::settings::scope::Scope;
use crate::settings::wait::{Note, until_ok};
use crate::storage::journal::Level;

use super::ctx::Ctx;
use super::deposit_addr::{self, DepositAddrs};
use super::deposit_plan::{Plan, plan, stops_round, transient};
use super::wallet::Networks;

/// Сколько монет со сбоем показывать в записи журнала (остальные — числом). Не настройка: формат текста записи.
const SHOW_FAILED: usize = 10;
/// Настройки круга, смена которых прерывает паузу (префиксы имён для Watch).
const WATCHED: [&str; 3] = [
    "venue.deposit_info_",
    "venue.bsc_network",
    "venue.deposit_min_absent_ok",
];

/// Итог одного круга.
#[derive(Default)]
struct Round {
    ok: usize,
    failed: Vec<String>,
    /// Монеты со сбоем временной причины (и не спрошенные после «секреты закрыты») — в повтор.
    retry: HashSet<String>,
    /// Не спрошены: круг прекращён (секреты закрыты).
    skipped: usize,
}

/// Чем закончилось ожидание.
enum Wake {
    /// Сменили настройки круга — сразу полный круг.
    Settings,
    /// Справочник Gate снова получен удачно — повтор монет со сбоем.
    Fetched,
    /// Срок полного круга.
    Timer,
    /// Рассылка настроек закрыта — система останавливается.
    Closed,
}

/// Что делает: бесконечный круг по монетам подписки (см. шапку). Принимает: окружение, справочник сетей, кэш адресов,
/// монеты подписки. Возвращает: только при остановке системы. Ошибки запросов — в итог круга, круг идёт дальше.
pub async fn refresh_loop(ctx: Ctx, nets: Networks, addrs: DepositAddrs, assets: Vec<Asset>) {
    let mut retry: Option<HashSet<String>> = None;
    let mut next_full = Instant::now();
    loop {
        let read = || {
            Ok((
                ctx.int("venue.deposit_info_requests_per_sec")?,
                ctx.int("venue.deposit_info_refresh_s")?,
                ctx.text("venue.bsc_network")?,
            ))
        };
        let note = Note {
            journal: &ctx.journal,
            kind: "venue.deposit_info",
            venue: Some(super::ctx::ID),
        };
        let Some((rate, period, bsc)) = until_ok(
            &ctx.settings,
            note,
            "Сведения депозита Gate (минимум)",
            read,
        )
        .await
        else {
            return;
        };
        // Подписка на настройки и метка справочника — до круга: изменение во время круга не теряется.
        let names = WATCHED.iter().map(|s| s.to_string()).collect();
        let mut w = Watch::new(ctx.settings.subscribe(), names);
        let generation = nets.fetched_gen();
        let full = retry.is_none();
        if full {
            next_full = Instant::now() + Duration::from_secs(period);
        }
        let dir = nets.latest();
        let known = |t: &str| nets.min_known(t);
        let p = plan(
            &assets,
            dir.as_deref().map(Vec::as_slice),
            &bsc,
            &known,
            retry.as_ref(),
        );
        let gap = Duration::from_secs_f64(1.0 / rate.max(1) as f64);
        let r = round(&ctx, &nets, &addrs, &p.ask, &bsc, gap).await;
        let level = if r.failed.is_empty() {
            Level::Info
        } else {
            Level::Warn
        };
        let msg = summary(full, &p, &r);
        let _ = ctx
            .journal
            .write(level, "venue.deposit_info", Some(super::ctx::ID), msg, None)
            .await;
        retry = (!r.retry.is_empty()).then_some(r.retry);
        match wait(&nets, &mut w, generation, retry.is_some(), next_full).await {
            Wake::Closed => return,
            Wake::Settings | Wake::Timer => retry = None,
            Wake::Fetched => {}
        }
    }
}

/// Что делает: запросы адреса депозита по плану с паузой `gap` между ними. Принимает: окружение, справочник сетей,
/// кэш адресов, монеты, сеть BSC, паузу. Возвращает: итог круга. Ошибок нет (сбои — в итог).
async fn round(
    ctx: &Ctx,
    nets: &Networks,
    addrs: &DepositAddrs,
    ask: &[Asset],
    bsc: &str,
    gap: Duration,
) -> Round {
    let mut r = Round::default();
    for (i, a) in ask.iter().enumerate() {
        match deposit_addr::fetch(ctx, addrs, nets, a, bsc).await {
            Ok(_) => r.ok += 1,
            Err(e) => {
                if transient(e.code) {
                    r.retry.insert(a.ticker.clone());
                }
                r.failed.push(format!("{} ({})", a.ticker, e.message));
                if stops_round(e.code) {
                    let rest = &ask[i + 1..];
                    r.retry.extend(rest.iter().map(|b| b.ticker.clone()));
                    r.skipped = rest.len();
                    break;
                }
            }
        }
        tokio::time::sleep(gap).await;
    }
    r
}

/// Что делает: ждёт следующего круга — смены настроек круга, срока полного круга `until`, а при `retry` ещё и
/// удачного получения справочника Gate (уже было после метки `generation` — сразу). Принимает: справочник сетей,
/// подписку на настройки, метку, есть ли повтор, срок. Возвращает: причину пробуждения. Ошибок нет.
async fn wait(
    nets: &Networks,
    w: &mut Watch,
    generation: u64,
    retry: bool,
    until: Instant,
) -> Wake {
    let note = nets.fetched_note();
    // Ожидание создаётся до проверки метки: толчок между проверкой и ожиданием не теряется (notify_waiters).
    let fetched = note.notified();
    tokio::pin!(fetched);
    if retry && nets.fetched_gen() != generation {
        return Wake::Fetched;
    }
    loop {
        tokio::select! {
            c = w.next() => match c {
                None => return Wake::Closed,
                Some(c) if ours(&c.scope) => return Wake::Settings,
                Some(_) => continue,
            },
            _ = &mut fetched, if retry => return Wake::Fetched,
            _ = tokio::time::sleep_until(until) => return Wake::Timer,
        }
    }
}

/// Изменение относится к Gate: вся система или площадка gate. Ошибок нет.
fn ours(s: &Scope) -> bool {
    match s {
        Scope::Global => true,
        Scope::Venue(v) => v == super::ctx::ID,
        _ => false,
    }
}

/// Текст итога круга. Принимает: полный ли круг, план, итог. Возвращает: строку. Ошибок нет.
fn summary(full: bool, p: &Plan, r: &Round) -> String {
    let what = if full {
        ""
    } else {
        " (повтор после сбоя)"
    };
    let mut m = format!("Сведения депозита Gate{what}: получены по {} монетам", r.ok);
    if !r.failed.is_empty() {
        let shown: Vec<&str> = r
            .failed
            .iter()
            .take(SHOW_FAILED)
            .map(String::as_str)
            .collect();
        m.push_str(&format!(
            ", сбой у {}: {}",
            r.failed.len(),
            shown.join("; ")
        ));
        if r.failed.len() > SHOW_FAILED {
            m.push_str(&format!(" и ещё {}", r.failed.len() - SHOW_FAILED));
        }
    }
    if r.skipped > 0 {
        m.push_str(&format!("; круг прекращён, не спрошены ещё {}", r.skipped));
    }
    if p.closed > 0 || p.no_bsc > 0 {
        m.push_str(&format!(
            "; не спрашивались: ввод в сети BSC закрыт — {}, сети BSC нет — {}",
            p.closed, p.no_bsc
        ));
    }
    if !r.retry.is_empty() {
        m.push_str(&format!(
            "; повтор {} монет — как только справочник Gate снова получен (разблокировка секретов, связь)",
            r.retry.len()
        ));
    }
    m
}
