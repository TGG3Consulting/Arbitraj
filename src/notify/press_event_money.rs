//! Файл: notify/press_event_money.rs
//! Что делает: решение 408 (t193) — кнопки тревог с деньгами и исполнителем (продолжение press_event.rs — размер
//! файла). Всё считается СЕЙЧАС (балансы, сводка перевода, оценка газа), затем — вопрос «Да, …» / «Отмена»
//! (press_confirm.rs ask) и та же команда ядра, что у интерфейса:
//! - sb.<биржа>.<тикер> — «Продать» (SellBalance) со счёта, который адаптер считает торговым (account_role, как панель
//!   «Балансы»): доступность и количество — как у «Продать» «Балансов» (ctl/sell.rs);
//! - tu.<откуда>.<куда>.<сумма> — перевод USDT с заполненными откуда / куда / сколько (TransferUsdt): сводка ядра
//!   usdt_transfer_plan::quote (ctl/transfer_input.rs transfer_pending) — та же проверка, что у интерфейса;
//! - tt.<куда> — «Перевести USDT на …» (risk.low_usdt): новым сообщением — «Откуда?» (площадки со свободными USDT,
//!   куда из них можно перевести — usdt_sources::to_list) кнопками tf;
//! - tf.<откуда>.<куда> — «Сколько?»: можно вывести, комиссия, минимум; «Всё» и доли notify.transfer_pct — кнопками tu
//!   (другая сумма — диалог «Перевести USDT» чата «Управление»);
//! - xd.<площадка>.<кошелёк> — «Развернуть исполнитель» — notify/press_event_exec.rs.
//! Концы перевода — ключи End::key («биржа» или «dex/кошелёк»).
//! Кто вызывает: notify/press_event.rs (event_button).
//! От чего зависит: press_confirm.rs (ask), press_event.rs (pending), ctl/transfer_input.rs (transfer_pending),
//! ctl/transfer_press.rs (pcts), ctl/kb.rs, core (venue_balances, usdt_sources, usdt_transfer_plan, шлюз).
//! Настройки: notify.transfer_pct (доли), ui.balances_refresh_ms (через Core::balances_age).

use rust_decimal::Decimal;

use crate::core::types::{ManualAction, VenueId};
use crate::core::usdt_sources as src;
use crate::core::usdt_transfer_plan as plan;
use crate::storage::journal::Level;

use super::ctl::kb::{Screen, btn};
use super::ctl::transfer::n;
use super::ctl::transfer_input::transfer_pending;
use super::mirror_state::Pending;
use super::press::Ctx;
use super::press_event::{BAD, pending};
use super::tg_buttons::{CallbackQuery, CbMessage};

impl Ctx<'_> {
    /// Кнопки тревог с деньгами (sb, tu, tt, tf, xd). Принимает: нажатие, сообщение, что, данные. Ошибок нет.
    pub async fn money_button(&self, q: &CallbackQuery, msg: &CbMessage, op: &str, arg: &str) {
        let res = match op {
            "sb" => self.ev_sell(arg).await,
            "tu" => self.ev_transfer(arg).await,
            "xd" => self.ev_deploy(arg).await,
            "tt" | "tf" => {
                let s = if op == "tt" {
                    self.ev_from(arg).await
                } else {
                    self.ev_amount(arg).await
                };
                return match s {
                    Ok(s) => {
                        self.answer(&q.id, None).await;
                        self.send_step(msg, &s).await
                    }
                    Err(why) => self.answer(&q.id, Some(&why)).await,
                };
            }
            _ => return self.answer(&q.id, Some(BAD)).await,
        };
        match res {
            Ok(p) => self.ask(&q.id, Some(msg.message_id), p).await,
            Err(why) => self.answer(&q.id, Some(&why)).await,
        }
    }

    /// Шаг перевода новым сообщением (ответом на тревогу). Ошибок нет: сбой — в журнал.
    async fn send_step(&self, msg: &CbMessage, s: &Screen) {
        if let Err(e) = self
            .tg
            .send_markup(self.chat, &s.text, Some(s.markup()), Some(msg.message_id))
            .await
        {
            self.log(Level::Warn, format!("Перевод USDT: шаг не отправлен: {e}"))
                .await;
        }
    }

    /// «Продать» с торгового счёта биржи. Ошибки: текст для нажавшего.
    async fn ev_sell(&self, arg: &str) -> Result<Pending, String> {
        let (venue, ticker) = arg.split_once('.').ok_or(BAD)?;
        let b = self
            .core
            .venue_balances(&VenueId(venue.to_string()), self.core.balances_age())
            .await?;
        let (account, row) = b
            .accounts
            .into_iter()
            .filter(|a| a.trading)
            .find_map(|a| {
                let r = a.rows.into_iter().find(|r| r.asset == ticker)?;
                Some((a.account, r))
            })
            .ok_or_else(|| format!("На торговом счёте {venue} нет {ticker}"))?;
        if let Some(Err(why)) = &row.sell {
            return Err(format!("«Продать» недоступна: {why}"));
        }
        let q = format!(
            "Продать {} {ticker} со счёта «{account}» на {} по рынку? Всё свободное — в USDT, выручка — на торговый счёт.",
            n(row.free),
            super::human::venue(venue)
        );
        let action = ManualAction::SellBalance {
            venue: venue.into(),
            account,
            asset: ticker.into(),
        };
        Ok(pending(action, None, q))
    }

    /// Конец перевода по ключу End::key среди площадок сейчас. Ошибки: «площадки нет».
    async fn end(&self, key: &str) -> Result<crate::core::usdt_transfer_check::End, String> {
        let all = src::all(self.core, self.core.balances_age()).await;
        src::by_key(&all, key)
            .map(|s| s.end.clone())
            .ok_or_else(|| format!("Площадки {key} среди площадок перевода нет"))
    }

    /// Перевод с заполненными откуда / куда / сколько. Ошибки: текст для нажавшего.
    async fn ev_transfer(&self, arg: &str) -> Result<Pending, String> {
        let mut it = arg.splitn(3, '.');
        let (Some(f), Some(t), Some(a)) = (it.next(), it.next(), it.next()) else {
            return Err(BAD.into());
        };
        let a: Decimal = a.parse().map_err(|_| BAD.to_string())?;
        let (f, t) = (self.end(f).await?, self.end(t).await?);
        transfer_pending(self.core, &f, &t, Some(a)).await
    }

    /// «Откуда?» при выбранном «Куда». Ошибки: текст для нажавшего.
    async fn ev_from(&self, to: &str) -> Result<Screen, String> {
        let all = src::all(self.core, self.core.balances_age()).await;
        let dst = src::by_key(&all, to).ok_or_else(|| format!("Площадки {to} нет"))?;
        let mut s = Screen::new(format!("Перевести USDT на {} — откуда?", dst.label));
        let mut any = false;
        for x in src::from_list(&all) {
            if !src::to_list(&all, &x.end).iter().any(|y| y.end == dst.end) {
                continue;
            }
            let usdt = x.usdt.map(n).unwrap_or_default();
            let data = format!("e:tf.{}.{to}", x.end.key());
            s = s.one(btn(&format!("{} — {usdt} USDT", x.label), &data));
            any = true;
        }
        if !any {
            return Err("Площадок со свободными USDT, откуда можно перевести сюда, нет".into());
        }
        Ok(s)
    }

    /// «Сколько?»: сводка «Всё» и доли. Ошибки: текст для нажавшего.
    async fn ev_amount(&self, arg: &str) -> Result<Screen, String> {
        let (fk, tk) = arg.split_once('.').ok_or(BAD)?;
        let (f, t) = (self.end(fk).await?, self.end(tk).await?);
        let qt = plan::quote(self.core, &f, &t, None)
            .await
            .map_err(|e| format!("Перевести нельзя: {e}"))?;
        let all = src::all(self.core, self.core.balances_age()).await;
        let mut s = Screen::new(format!(
            "Перевести USDT с {} на {} — сколько?\nМожно вывести: {} {} (свободно {}), минимум вывода {} {}{}",
            src::label_of(&all, &f),
            src::label_of(&all, &t),
            n(qt.withdrawable),
            qt.ticker,
            n(qt.available),
            n(qt.min),
            qt.ticker,
            qt.note
                .as_ref()
                .map(|x| format!("\nУдержания: {x}"))
                .unwrap_or_default()
        ));
        let go = |a: Decimal| format!("e:tu.{fk}.{tk}.{}", n(a));
        let mut opts = vec![(format!("Всё ({})", n(qt.amount)), go(qt.amount))];
        for p in self.pcts().await {
            let a = qt.withdrawable * p / Decimal::ONE_HUNDRED;
            opts.push((format!("{}% ({})", n(p), n(a)), go(a)));
        }
        for (title, data) in opts {
            match btn(&title, &data) {
                Some(b) => s.rows.push(vec![b]),
                None => {
                    s = s.line(format!(
                        "«{title}» — только в интерфейсе: данные кнопки длиннее 64 байт"
                    ))
                }
            }
        }
        Ok(s.line("Другая сумма — чат «Управление» → «Перевести USDT»."))
    }
}
