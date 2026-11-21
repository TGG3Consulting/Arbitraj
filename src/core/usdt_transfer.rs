//! Файл: core/usdt_transfer.rs
//! Что делает: решение 176 (Тигран, 05.11.2026: «выбрал биржу С, выбрал биржу НА, количество USDT, нажал „Отправить“ —
//! он отправил») — команда ядра ManualAction::TransferUsdt: ручной перевод USDT биржа ↔ биржа ↔ кошелёк DEX. Не новая
//! логика перевода: проверка — usdt_transfer_plan.rs (та же, что у сводки), отправка — СУЩЕСТВУЮЩИЙ перегон USDT ядра
//! usdt_return::relay (ожидание баланса отправителя и повтор при откате — решение 108; вывод с повтором при «средства
//! заблокированы» — решение 166, transfer_locked.rs; ожидание зачисления и перевод на торговый счёт получателя —
//! transfer_credit.rs, решение 102; уведомления usdt.relayed / usdt.relay_failed). Ход перевода (проверка → отправлено →
//! в сети с хешем) — в памяти ядра (Inner::usdt_moves), его показывает экран Telegram, не спрашивая биржи.
//! Безопасность: с одного отправителя — один перевод за раз (повторное нажатие — «уже идёт»); метка снимается при
//! любом выходе (Drop). Журнал: core.command (кто, что, итог — commands.rs) и core.usdt_transfer (начало).
//! Кто вызывает: core/commands.rs (run_command), core/transfers.rs (stage — ход перевода).
//! От чего зависит: usdt_transfer_plan.rs, usdt_return.rs (relay), engine.rs.
//! Настройки: свои — нет (перегон — usdt_return.balance_wait_ms и др., вывод — transfer.locked_*).

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::core::types::ManualAction;
use crate::storage::journal::Level;

use super::engine::{Core, lk};
use super::usdt_transfer_check::End;
use super::usdt_transfer_plan as plan;

/// Ход перевода.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stage {
    /// Проверка и ожидание вывода биржей.
    Checking,
    /// Вывод принят отправителем, ждём хеш транзакции.
    Sent,
    /// Транзакция в сети — ждём зачисления.
    OnChain { tx: String },
}

/// Идущий перевод.
#[derive(Debug, Clone)]
pub struct Move {
    /// ID перевода (к ID вывода биржи): «tu<время>».
    pub id: String,
    pub to: String,
    pub amount: Decimal,
    pub stage: Stage,
    pub at: DateTime<Utc>,
}

/// Метка «идёт перевод с отправителя»: снимается при выходе из любой ветки.
struct Mark {
    core: Core,
    key: String,
}

impl Drop for Mark {
    fn drop(&mut self) {
        lk(&self.core.inner.usdt_moves).remove(&self.key);
    }
}

/// Команда «Перевести USDT». Принимает: ядро, действие, кто. Возвращает: None — действие не отсюда; Some(итог).
pub async fn command(core: &Core, a: &ManualAction, who: &str) -> Option<Result<String, String>> {
    let ManualAction::TransferUsdt {
        from,
        from_wallet,
        to,
        to_wallet,
        amount,
    } = a
    else {
        return None;
    };
    Some(
        run(
            core,
            (from, from_wallet.as_deref()),
            (to, to_wallet.as_deref()),
            amount,
            who,
        )
        .await,
    )
}

/// Проверка и перевод. Ошибки: текст причины (проверка, «уже идёт», перегон не состоялся).
async fn run(
    core: &Core,
    from: (&str, Option<&str>),
    to: (&str, Option<&str>),
    amount: &str,
    who: &str,
) -> Result<String, String> {
    let want: Decimal = amount
        .trim()
        .replace(',', ".")
        .parse()
        .map_err(|_| format!("сумма «{amount}» — не число"))?;
    let (f, t) = (
        plan::end(core, from.0, from.1)?,
        plan::end(core, to.0, to.1)?,
    );
    let id = format!("tu{}", Utc::now().format("%Y%m%d%H%M%S%3f"));
    let _mark = mark(core, &f, &t, &id, want)?;
    let q = plan::quote(core, &f, &t, Some(want)).await?;
    let acc = plan::account(&f, &t)?;
    let line = format!(
        "Перевод USDT {id}: {} {} {} → {}, нажал {who}",
        q.amount,
        q.ticker,
        f.key(),
        t.key()
    );
    core.inner
        .deps
        .log(
            Level::Info,
            "core.usdt_transfer",
            Some(&f.venue.0),
            line,
            None,
        )
        .await;
    let got = super::usdt_return::relay(core, &id, &f.venue, &t.venue, acc.as_ref(), q.amount)
        .await
        .map_err(|e| e.to_string())?
        .credited;
    Ok(format!(
        "{} {} {} → {}: зачислено {got} {}",
        q.amount,
        q.ticker,
        f.key(),
        t.key(),
        q.ticker
    ))
}

/// Ставит метку отправителя. Ошибка: с него уже идёт перевод.
fn mark(core: &Core, f: &End, t: &End, id: &str, amount: Decimal) -> Result<Mark, String> {
    let key = f.key();
    let mut m = lk(&core.inner.usdt_moves);
    if let Some(x) = m.get(&key) {
        return Err(format!(
            "с {key} уже идёт перевод {} ({} USDT) — дождитесь итога",
            x.id, x.amount
        ));
    }
    let mv = Move {
        id: id.into(),
        to: t.key(),
        amount,
        stage: Stage::Checking,
        at: Utc::now(),
    };
    m.insert(key.clone(), mv);
    Ok(Mark {
        core: core.clone(),
        key,
    })
}

/// Ход перевода по ID (вызывает transfers.rs для любого перевода; не ручной перевод — ничего). Ошибок нет.
pub fn stage(core: &Core, id: &str, s: Stage) {
    if let Some(m) = lk(&core.inner.usdt_moves).values_mut().find(|m| m.id == id) {
        m.stage = s;
    }
}

impl Core {
    /// Идущий ручной перевод с отправителя (ключ End::key). None — не идёт. Ошибок нет.
    pub fn usdt_move(&self, from_key: &str) -> Option<Move> {
        lk(&self.inner.usdt_moves).get(from_key).cloned()
    }
}
