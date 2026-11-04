//! Файл: venues/gate/deposit_info.rs
//! Что делает: фоновое обновление сведений депозита Gate по монетам подписки (решение 143). Минимум депозита Gate
//! отдаёт только в ответе адреса депозита (min_deposit_amount), а не в справочниках монет; раньше адрес спрашивался
//! только перед сделкой, а до сделки не доходило из-за неизвестного минимума (отказ «минимальный депозит неизвестен»).
//! Теперь по каждой монете подписки адрес депозита в сети BSC запрашивается фоном с паузой между запросами, минимум и
//! подтверждения сети попадают в справочник сетей (wallet.rs, networks_parse::Extra) — ядро видит их со следующим
//! обновлением справочника (venue.networks_fresh_ms). Годный адрес заодно попадает в кэш адресов (deposit_addr.rs).
//! Итог круга — одна запись в журнал (сколько получено, у каких монет сбой — первые из списка).
//! Кто вызывает: subs.rs (запуск при новом списке монет подписки, как обновление комиссий — решение 131).
//! От чего зависит: deposit_addr.rs, wallet.rs (Networks), ctx.rs, settings/wait.rs.
//! Настройки: venue.deposit_info_requests_per_sec [gate], venue.deposit_info_refresh_s [gate], venue.bsc_network [gate].
//! Не заданы — круг ждёт настройку (запись в журнал) и продолжает сам, когда их зададут.
//!
//! Официальная документация https://www.gate.com/docs/developers/apiv4/en/ : «Generate currency deposit address»
//! GET /wallet/deposit_address?currency= (min_deposit_amount, multichain_addresses[].min_confirms); лимит частоты —
//! таблица «Frequency limit rule», строка Wallet: «The Others: 200r/10s per endpoint», считается по UID.

use std::time::Duration;

use crate::core::types::Asset;
use crate::settings::wait::{Note, until_ok};
use crate::storage::journal::Level;

use super::ctx::Ctx;
use super::deposit_addr::{self, DepositAddrs};
use super::wallet::Networks;

/// Сколько монет со сбоем показывать в записи журнала (остальные — числом). Не настройка: формат текста записи.
const SHOW_FAILED: usize = 10;

/// Что делает: бесконечный круг по монетам подписки — запрос адреса депозита каждой (минимум и подтверждения в
/// справочник), пауза 1 / venue.deposit_info_requests_per_sec между запросами, после круга — запись в журнал и пауза
/// venue.deposit_info_refresh_s. Принимает: окружение, справочник сетей, кэш адресов, монеты подписки.
/// Возвращает: только при остановке системы (рассылка настроек закрыта). Ошибки запросов — в итог круга, круг идёт дальше.
pub async fn refresh_loop(ctx: Ctx, nets: Networks, addrs: DepositAddrs, assets: Vec<Asset>) {
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
        let gap = Duration::from_secs_f64(1.0 / rate.max(1) as f64);
        let mut ok = 0usize;
        let mut failed: Vec<String> = Vec::new();
        for a in &assets {
            match deposit_addr::fetch(&ctx, &addrs, &nets, a, &bsc).await {
                Ok(_) => ok += 1,
                Err(e) => failed.push(format!("{} ({})", a.ticker, e.message)),
            }
            tokio::time::sleep(gap).await;
        }
        let msg = summary(ok, &failed);
        let level = if failed.is_empty() {
            Level::Info
        } else {
            Level::Warn
        };
        let _ = ctx
            .journal
            .write(level, "venue.deposit_info", Some(super::ctx::ID), msg, None)
            .await;
        tokio::time::sleep(Duration::from_secs(period)).await;
    }
}

/// Текст итога круга. Принимает: сколько монет получено, сбои «монета (причина)». Возвращает: строку. Ошибок нет.
fn summary(ok: usize, failed: &[String]) -> String {
    let mut m = format!("Сведения депозита Gate: получены по {ok} монетам");
    if !failed.is_empty() {
        let shown: Vec<&str> = failed
            .iter()
            .take(SHOW_FAILED)
            .map(String::as_str)
            .collect();
        m.push_str(&format!(", сбой у {}: {}", failed.len(), shown.join("; ")));
        if failed.len() > SHOW_FAILED {
            m.push_str(&format!(" и ещё {}", failed.len() - SHOW_FAILED));
        }
    }
    m
}
