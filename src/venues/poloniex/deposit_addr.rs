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

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use crate::core::types::Asset;
use crate::gateway::addr_claim::{self, Missing};
use crate::gateway::addr_create::Creating;
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::deposit_addr_new;
use super::networks_parse::{PAYMENT_ID, chain_coin};
use super::wallet;

/// Полученные адреса: (монета, blockchain) → адрес; память создания (ревью t165 Н2, Н4). Дешёво клонируется.
#[derive(Clone, Default)]
pub struct DepositAddrs {
    map: Arc<RwLock<HashMap<(String, String), String>>>,
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
        if let Some(a) = self.map.read().unwrap_or_else(|p| p.into_inner()).get(&key) {
            return Ok(a.clone());
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
        let got = deposit_addr_new::list(ctx, &coin).await?;
        // Решение 379: адреса в ответе нет — создать (предпроверка, перевод) или NotApplicable (прогрев без «да»).
        if let Some(a) = deposit_addr_new::pick(&got, &coin) {
            self.keep(key, &a);
            return Ok(a);
        }
        if addr_claim::on_missing() == Missing::Forbidden {
            let m = addr_claim::forbidden_text("Poloniex", &asset.ticker, chain);
            return Err(ctx.tag(GatewayError::new(ErrorCode::NotApplicable, None, m)));
        }
        // Ревью t165 Н4: одно создание за раз на (монета, сеть); дождавшийся берёт итог первого (кэш или неудача).
        let _pass = self.creating.enter(&key).await;
        if let Some(a) = self.map.read().unwrap_or_else(|p| p.into_inner()).get(&key) {
            return Ok(a.clone());
        }
        // Ревью t165 Н2: неудачное создание не повторяется до срока venue.networks_fresh_ms (не задана — до
        // перезапуска); причина биржи видна в отказе.
        let keep = ctx
            .int("venue.networks_fresh_ms")
            .ok()
            .map(Duration::from_millis);
        if let Some(mut e) = self.creating.failed(&key, keep) {
            e.message = format!(
                "{} (создание не повторяется: {})",
                e.message,
                retry_text(keep)
            );
            return Err(e);
        }
        match deposit_addr_new::create(ctx, &asset.ticker, chain, &coin).await {
            Ok(a) => {
                self.creating.ok(&key);
                self.keep(key, &a);
                Ok(a)
            }
            Err(e) => {
                self.creating.fail(&key, &e);
                Err(e)
            }
        }
    }

    /// Адрес — в кэш. Ошибок нет.
    fn keep(&self, key: (String, String), a: &str) {
        self.map
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .insert(key, a.to_string());
    }
}

/// Когда создание будет повторено (ревью t165 Н2). Принимает: срок. Возвращает: текст. Ошибок нет.
fn retry_text(keep: Option<Duration>) -> String {
    match keep {
        Some(d) => format!(
            "повтор не раньше чем через {} мс (venue.networks_fresh_ms)",
            d.as_millis()
        ),
        None => "до перезапуска — venue.networks_fresh_ms не задана".into(),
    }
}

/// Ошибка InvalidAddress с площадкой Poloniex.
fn invalid(ctx: &Ctx, msg: String) -> GatewayError {
    ctx.tag(GatewayError::new(ErrorCode::InvalidAddress, None, msg))
}
