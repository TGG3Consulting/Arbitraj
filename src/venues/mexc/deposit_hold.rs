//! Файл: venues/mexc/deposit_hold.rs
//! Что делает: удержания вывода MEXC (решение 150, Тигран 04.11.2026) — предварительно зачисленные депозиты (статус
//! 9 PRE_SUCCESS). Справка MEXC: такие депозиты «can be used for trading but cannot be withdrawn» до полного числа
//! подтверждений. Живой случай 04.11 (сделка 20261104173851521BABYDOGE, mexc → bsc_dex): покупка прошла, вывод — отказ
//! «код 16021: Some pre-crediting assets are temporarily unavailable for withdrawal. Please reconfirm the quantity.»
//! Как именно MEXC ограничивает вывод при PRE_SUCCESS, в документации НЕТ — правило вычета решает ядро
//! (core/precheck_withdraw.rs); адаптер только отдаёт список таких депозитов.
//! Кто вызывает: mod.rs (withdraw_hold из контракта).
//! От чего зависит: rest.rs, num.rs, ctx.rs, errors.rs, core/types (WithdrawHold).
//! Настройки: нет своих.
//!
//! Официальная документация: «Deposit History(supporting network)» — GET /api/v3/capital/deposit/hisrec
//! https://www.mexc.com/api-docs/spot-v3/wallet-endpoints/deposit-historysupporting-network — права SPOT_WITHDRAW_READ;
//! вес IP 10 по разбору документации docs/notes/mexc-audit-2026-11-03.md и README.md папки (в задании решения 150
//! указан вес 1 — расхождение, сверить); корзина IP 300 веса / 10 с. Параметры coin, status, startTime, endTime, limit
//! необязательны; адаптер (как deposits.rs) окна не задаёт — берутся умолчания API: последние 7 дней, limit 1000.
//! Поля записи: amount, coin, network, status, confirmTimes, unlockConfirm (формат confirmTimes в документации не
//! описан — показывается как есть).

use reqwest::Method;
use serde_json::Value;

use crate::core::types::{HoldItem, WithdrawHold};
use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::errors::{self, Op};
use super::{num, rest};

/// Статус 9 PRE_SUCCESS «Deposit History(supporting network)» — предварительно зачислено. Значение задано
/// документацией MEXC (перечисление status), это не настройка.
const PRE_SUCCESS: i64 = 9;

/// Удержания вывода сейчас: все депозиты со статусом 9 за окно API по умолчанию (последние 7 дней).
/// Возвращает: список удержаний (пусто — предварительно зачисленных нет). Ошибки: ключ, права, связь, лимит частоты,
/// непонятный ответ (BadResponse).
pub async fn withdraw_hold(ctx: &Ctx) -> GatewayResult<WithdrawHold> {
    let data = rest::signed(
        ctx,
        Method::GET,
        "/api/v3/capital/deposit/hisrec",
        vec![],
        Op::Other,
    )
    .await?;
    parse(&data)
}

/// Разбор ответа deposit/hisrec в удержания: только записи со status = 9. Принимает: ответ (массив записей).
/// Возвращает: удержания. Ошибки: BadResponse — ответ не массив, у записи нет status / coin / amount или они не числа.
pub fn parse(data: &Value) -> GatewayResult<WithdrawHold> {
    let list = data
        .as_array()
        .ok_or_else(|| errors::bad("история депозитов не массив", &data.to_string()))?;
    let mut items = vec![];
    for rec in list {
        if num::int(rec, "status")? != PRE_SUCCESS {
            continue;
        }
        let asset = num::text(rec, "coin")?;
        let amount = num::dec(rec, "amount")?;
        items.push(HoldItem {
            reason: reason(rec, &asset, amount),
            asset,
            amount,
        });
    }
    Ok(WithdrawHold { items })
}

/// Текст для человека: «предварительно зачислено 45 USDT (подтверждений 12, нужно для вывода 20, сеть BSC)».
/// Поля, которых нет в записи, — «не указано». Ошибок нет.
fn reason(rec: &Value, asset: &str, amount: rust_decimal::Decimal) -> String {
    let field = |f: &str| num::text_opt(rec, f).unwrap_or_else(|| "не указано".into());
    format!(
        "предварительно зачислено {amount} {asset} (подтверждений {}, нужно для вывода {}, сеть {})",
        field("confirmTimes"),
        field("unlockConfirm"),
        field("network")
    )
}
