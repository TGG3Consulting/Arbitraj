//! Файл: venues/phemex/networks_crawl.rs
//! Что делает: фоновый круг сбора справочника сетей Phemex по одной монете: монеты спот-пар к venue.quote_asset и сама
//! котируемая монета, по два подписанных запроса на монету (chainCfg и asset/info — wallet.rs), в темпе
//! venue.networks_requests_per_min (группа лимитов Others — 100 в минуту на пользователя, её же делят балансы, вывод и
//! история). Круг за кругом без перерыва; минимум депозита (minAmountRv) приходит этим же кругом заранее, не в момент
//! сделки (урок 21). Итог круга — одна запись в журнал при изменении (сколько монет собрано, какие ошибки у каких).
//! Кто вызывает: wallet.rs (первый coin_networks запускает круг; круг живёт до конца процесса).
//! От чего зависит: wallet.rs (coin_networks_of), symbols.rs, ctx.rs, settings/wait.rs.
//! Настройки: venue.networks_requests_per_min, venue.quote_asset [phemex]; через wallet.rs — venue.bsc_network,
//! venue.deposit_min_absent_ok.
//!
//! Официально (General API information → Rate limits, https://phemex-docs.github.io/#rest-api-ratelimit-rules): группа
//! Others — «100/minute»; «Every API consume request shares at its own weight» (вес справочников сетей не указан —
//! считается 1, как у всех, кого нет в таблице весов). Справочника всех монет с сетями одним запросом в документации нет.

use std::collections::BTreeMap;
use std::time::Duration;

use crate::settings::wait::{Note, until_ok};
use crate::storage::journal::Level;

use super::ctx::{Ctx, ID};
use super::symbols::Symbols;
use super::wallet::{Networks, coin_networks_of};

/// Код записей журнала.
const CODE: &str = "venue.networks";
/// Запросов на монету: настройки депозита и настройки вывода.
const REQS_PER_COIN: f64 = 2.0;

/// Работает до конца процесса. Ошибок не возвращает: всё — в журнал (одна запись на изменение итога круга).
pub async fn run(ctx: Ctx, symbols: Symbols, nets: Networks) {
    let mut told = String::new();
    loop {
        let note = Note {
            journal: &ctx.journal,
            kind: CODE,
            venue: Some(ID),
        };
        let read = || ctx.int("venue.networks_requests_per_min");
        let Some(per_min) =
            until_ok(&ctx.settings, note, "Сбор справочника сетей Phemex", read).await
        else {
            return;
        };
        let gap = Duration::from_secs_f64(60.0 * REQS_PER_COIN / per_min.max(1) as f64);
        let coins = match coins(&ctx, &symbols).await {
            Ok(c) => c,
            Err(m) => {
                say(
                    &ctx,
                    &mut told,
                    Level::Warn,
                    format!("Справочник сетей Phemex не собирается: {m}"),
                )
                .await;
                tokio::time::sleep(gap).await;
                continue;
            }
        };
        let mut errs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for c in &coins {
            if let Err(e) = coin_networks_of(&ctx, &nets, c).await {
                errs.entry(e.message).or_default().push(c.clone());
            }
            tokio::time::sleep(gap).await;
        }
        let failed: usize = errs.values().map(Vec::len).sum();
        let mut text = format!(
            "Справочник сетей Phemex: круг {} монет, в памяти {}, ошибок {failed}",
            coins.len(),
            nets.len()
        );
        for (why, list) in &errs {
            text.push_str(&format!("; {why}: {}", list.join(", ")));
        }
        let level = if failed == 0 {
            Level::Info
        } else {
            Level::Warn
        };
        say(&ctx, &mut told, level, text).await;
    }
}

/// Монеты круга: базовые монеты спот-пар к котируемой и сама котируемая. Ошибка — текст причины.
async fn coins(ctx: &Ctx, symbols: &Symbols) -> Result<Vec<String>, String> {
    symbols.ensure_fresh(ctx).await.map_err(|e| e.message)?;
    let quote = ctx.text("venue.quote_asset").map_err(|e| e.message)?;
    let mut list: Vec<String> = symbols.all().keys().cloned().collect();
    list.sort();
    if !list.contains(&quote) {
        list.push(quote);
    }
    Ok(list)
}

/// Запись в журнал, только если текст изменился. Ошибок нет.
async fn say(ctx: &Ctx, told: &mut String, level: Level, text: String) {
    if *told != text {
        ctx.log(level, CODE, text.clone()).await;
        *told = text;
    }
}
