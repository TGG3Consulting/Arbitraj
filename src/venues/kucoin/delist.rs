//! Файл: venues/kucoin/delist.rs
//! Что делает: делистинги KuCoin (решение 226, контракт delistings) — два источника:
//! (1) справочник пар: пары с котируемой монетой venue.quote_asset и меткой st = true («Special Treatment») — запись
//!    с признаком st (блок только при delist.st_block KuCoin = да, уточнение Тиграна 08.11.2026);
//! (2) анонсы делистинга (delist_ann.rs): монеты из заголовка или со страницы анонса и время делистинга из annDesc
//!    («at 8:00 on November 5, 2026 (UTC)») — ядро блокирует за delist.block_hours до него.
//! Сбой анонсов не отменяет справочник — причина идёт в warnings ответа.
//! Кто вызывает: mod.rs (Venue::delistings), tests/delist_sources.rs.
//! От чего зависит: rest.rs, num.rs, errors.rs, ctx.rs, reply.rs (Pool), delist_ann.rs, gateway/delist_ann.rs.
//! Настройки: venue.quote_asset; delist.ann_max_age_h, delist.ann_max_pages (анонсы).
//!
//! Официальная документация: Get All Symbols GET /api/v2/symbols (Public, вес 4)
//! https://www.kucoin.com/docs-new/api-3470154 — «st boolean required Whether it is a Special Treatment symbol».
//! Связь ST с делистингом — живой анонс 05.11.2026 «KuCoin Will Delist Multiple Tokens on 2026-11-05»: «In accordance
//! with the Special Treatment Rules of KuCoin, the following projects will be delisted…» (тот самый делистинг ION,
//! решение 175). Живой ответ 08.11: 955 пар, st = true у 22 (POLS, SOUL, P2P, AGLD, …).

use std::collections::HashMap;

use serde_json::Value;

use crate::core::types::{DelistItem, Delistings};
use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::num;
use super::reply::Pool;

/// Источник для человека.
const SOURCE: &str =
    "KuCoin GET /api/v2/symbols: st = true (Special Treatment — зона риска делистинга)";

/// Что делает: справочник пар (st) + анонсы делистинга. Возвращает: Delistings по фактам API.
/// Ошибки: связь или ответ KuCoin у справочника, не задана venue.quote_asset, неверный формат справочника.
pub async fn delistings(ctx: &Ctx) -> GatewayResult<Delistings> {
    let quote = ctx.text("venue.quote_asset")?;
    let data = super::rest::public(ctx, "/api/v2/symbols", vec![], Pool::Public).await?;
    let (base, known) = parse(&data, &quote)?;
    let ann = super::delist_ann::items(ctx, &known, &quote).await;
    Ok(crate::gateway::delist_ann::merge(base, ann))
}

/// Что делает: разбор data ответа GET /api/v2/symbols. Принимает: data, котируемую монету. Возвращает: (пары со
/// st = true, справочник «тикер → символ» с котируемой монетой quote — для сверки анонсов). Ошибка BadResponse — data
/// не массив, нет baseCurrency/symbol.
pub fn parse(data: &Value, quote: &str) -> GatewayResult<(Delistings, HashMap<String, String>)> {
    let list = data
        .as_array()
        .ok_or_else(|| super::errors::bad("symbols: data не массив", ""))?;
    let (mut items, mut known) = (vec![], HashMap::new());
    for s in list {
        if num::text_opt(s, "quoteCurrency").as_deref() != Some(quote) {
            continue;
        }
        let ticker = num::text(s, "baseCurrency")?;
        let symbol = num::text(s, "symbol")?;
        known.insert(ticker.clone(), symbol.clone());
        if !matches!(s.get("st"), Some(Value::Bool(true))) {
            continue;
        }
        items.push(DelistItem {
            ticker,
            symbol,
            trading_end: None,
            withdraw_end: None,
            source: SOURCE.into(),
            st: true,
        });
    }
    Ok((Delistings::of(items), known))
}
