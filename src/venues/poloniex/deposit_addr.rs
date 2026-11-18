//! Файл: venues/poloniex/deposit_addr.rs
//! Что делает: адрес депозита Poloniex по монете и сети (blockchain). Кэш в памяти на время процесса (адрес нужен в
//! предпроверке каждой сделки до покупки — урок 25, решение 118). Сеть с memo (address-payment-id) — ошибка «требует
//! memo — не поддерживается»; такой адрес не кэшируется.
//! Кто вызывает: mod.rs (deposit_address).
//! От чего зависит: rest.rs, wallet.rs (one_raw), networks_parse.rs (chain_coin), deposit_addr_new.rs (создание,
//! проверка адреса), gateway/addr_claim.rs, ctx.rs.
//! Настройки: нет своих; venue.networks_fresh_ms [poloniex] — срок памяти неудачного создания (ревью t165 Н2).
//!
//! Официально (https://api-docs.poloniex.com/spot/api/private/wallet): Deposit Addresses GET /wallets/addresses?currency=
//! — ответ объект «имя монеты → адрес» («50» на VIP0); параметра сети у запроса НЕТ. Для монеты с несколькими сетями
//! имя монеты в нужной сети — поле coin сети в /v2/currencies/{currency} (networkList; в справочнике v1 такие имена —
//! «USDTETH», «USDTTRON») — по нему и запрашивается адрес (проверить на живом ответе для сети BSC).
//! Решение 379 (задача t165, Тигран 17.11): адреса нет — бот создаёт его сам, New Currency Address POST /wallets/address
//! с тем же именем монеты сети (deposit_addr_new.rs); прогрев адресов без venue.deposit_warm_create = да не создаёт —
//! NotApplicable (gateway/addr_claim.rs, как KuCoin). Раньше — отказ «создайте адрес в кабинете Poloniex» (живой факт
//! 17.11: ESIM bsc_dex → poloniex — 94 отказа предпроверки подряд, BANK → poloniex — 6, сделки потеряны).
//! Ревью t165: адрес после создания — только из повторного GET (Б1), POST не повторяется на резерве после таймаута (Н1),
//! неудача создания помнится до срока venue.networks_fresh_ms (Н2), одно создание за раз на (монета, сеть) (Н4) —
//! deposit_addr_new.rs, gateway/addr_create.rs.
//! Счёт зачисления — спот: «accountType … SPOT is the default and only supported one» (Account Information).

use std::time::Duration;

use crate::core::types::Asset;
use crate::gateway::addr_book::AddrBook;
use crate::gateway::addr_claim::{self, Missing};
use crate::gateway::addr_create::{Creating, Failure, retry_text};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::storage::journal::Level;

use super::ctx::Ctx;
use super::deposit_addr_new;
use super::networks_parse::{PAYMENT_ID, chain_coin, coin_clash};
use super::wallet;

/// Полученные адреса: (монета, blockchain) → адрес; память создания (ревью t165 Н2, Н4). Дешёво клонируется.
/// Решение 411: ключ памяти — ещё и отпечаток ключа API биржи (смена ключа — старый адрес не берётся).
#[derive(Clone, Default)]
pub struct DepositAddrs {
    book: AddrBook<String>,
    creating: Creating,
}

impl DepositAddrs {
    /// Что делает: адрес депозита монеты в сети — из памяти или запросом.
    /// Принимает: окружение, монету, blockchain. Возвращает: адрес.
    /// Официально: Deposit Addresses GET /wallets/addresses, New Currency Address POST /wallets/address
    /// (https://api-docs.poloniex.com/spot/api/private/wallet).
    /// Ошибки: InvalidAddress — сети нет, сеть с memo, созданный адрес не подтверждён повторным GET; NotApplicable — адреса нет, а
    /// создание запрещено признаком задачи (прогрев, gateway/addr_claim.rs); отказ Poloniex в создании — его код и
    /// сообщение (deposit_addr_new.rs); связь, коды Poloniex.
    pub async fn get(&self, ctx: &Ctx, asset: &Asset, chain: &str) -> GatewayResult<String> {
        let key = (asset.ticker.clone(), chain.to_string());
        // Решение 411: память — под отпечатком ключа API биржи и по режиму задачи (gateway/addr_book.rs).
        let mark = ctx.vault.mark(super::ctx::SECRET_API_KEY);
        if let Some(a) = self.book.lookup(mark, &asset.ticker, chain)? {
            return Ok(a);
        }
        let data = wallet::one_raw(ctx, &asset.ticker).await?;
        let (coin, ctype) = chain_coin(&data, &asset.ticker, chain).ok_or_else(|| {
            let m = format!(
                "У {} нет сети {chain} в справочнике Poloniex /v2/currencies",
                asset.ticker
            );
            invalid(ctx, m)
        })?;
        if ctype == PAYMENT_ID {
            let m = format!(
                "Депозит Poloniex {} в сети {chain} требует memo (currencyType = {PAYMENT_ID}) — не поддерживается",
                asset.ticker
            );
            return Err(invalid(ctx, m));
        }
        if coin.is_empty() {
            let m = format!(
                "Poloniex: у сети {chain} монеты {} нет поля coin",
                asset.ticker
            );
            return Err(invalid(ctx, m).with_raw(data.to_string()));
        }
        // Ревью 2 t165: имя монеты сети BSC совпадает с именем другой сети этой монеты — по ответу GET (сети в нём нет)
        // не понять, к какой сети привязан адрес: ни принимать адрес, ни создавать (POST) нельзя.
        if let Some(other) = coin_clash(&data, &asset.ticker, chain) {
            let m = format!(
                "Poloniex {}: имя монеты сети {chain} «{coin}» неоднозначно (совпадает с сетью {other}) — адрес не \
                 принимаю",
                asset.ticker
            );
            return Err(invalid(ctx, m));
        }
        let got = deposit_addr_new::list(ctx, &coin).await?;
        // Решение 379: адреса в ответе нет — создать (предпроверка, перевод) или NotApplicable (прогрев без «да»).
        if let Some(a) = deposit_addr_new::pick(&got, &coin) {
            self.keep(mark, key, &a);
            return Ok(a);
        }
        if addr_claim::on_missing() == Missing::Forbidden {
            let m = addr_claim::forbidden_text("Poloniex", &asset.ticker, chain);
            return Err(ctx.tag(GatewayError::new(ErrorCode::NotApplicable, None, m)));
        }
        // Ревью t165 Н4: одно создание за раз на (монета, сеть); дождавшийся берёт итог первого (кэш или неудача).
        let _pass = self.creating.enter(&key).await;
        // Создал ли адрес тот, кого ждали (память без учёта режима: это ответ биржи, полученный сейчас).
        if let Some(a) = self.book.peek(mark, &asset.ticker, chain) {
            return Ok(a);
        }
        // Ревью t165 Н2 и ревью 2 Н-А: отказ биржи не повторяется до срока venue.networks_fresh_ms (не задана — до
        // перезапуска), отправленное без подтверждения создание — до перезапуска; причина видна в отказе.
        let keep = ctx
            .int("venue.networks_fresh_ms")
            .ok()
            .map(Duration::from_millis);
        if let Some(e) = self.creating.failed(&key, keep) {
            return Err(e);
        }
        match deposit_addr_new::create(ctx, &asset.ticker, chain, &coin).await {
            Ok(a) => {
                self.creating.ok(&key);
                self.keep(mark, key, &a);
                Ok(a)
            }
            Err(f) => {
                // Ревью 2 Н-А: одна запись Warn на ключ — создание отправлено, адреса в списке нет.
                if self.creating.fail(&key, &f) && f.kind == Failure::Sent {
                    let m = format!(
                        "Poloniex {} в сети {chain}: {} — {}",
                        asset.ticker,
                        f.err.message,
                        retry_text(Failure::Sent, keep)
                    );
                    ctx.log(Level::Warn, "venue.deposit_address", m).await;
                }
                Err(*f.err)
            }
        }
    }

    /// Адрес — в память под отпечатком ключа API (решение 411). Ошибок нет.
    fn keep(&self, mark: Option<u64>, key: (String, String), a: &str) {
        self.book.keep(mark, key.0, key.1, a.to_string());
    }
}

/// Ошибка InvalidAddress с площадкой Poloniex.
fn invalid(ctx: &Ctx, msg: String) -> GatewayError {
    ctx.tag(GatewayError::new(ErrorCode::InvalidAddress, None, msg))
}
