//! Получает индивидуальные условия по явному запросу выбранного кошелька и проверяет обмен в дочернем блоке.
mod cash;
mod registry;
mod wire;
use super::{white_state, zeroex};
use crate::gateway::{
    errors::{ErrorCode, GatewayError, GatewayResult},
    http,
    source_quote::{FirmTerms, Preview, Request},
};
use crate::venues::bsc_dex::{
    check::simulate::{self, Call},
    ctx::Ctx,
    node::{abi::IERC20, rpc},
    num, wallet,
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, U64, U256, keccak256};
use reqwest::{Client, Url, header::HeaderValue};
use serde_json::{Value, json};

pub(super) struct RequestApi {
    pub client: Client,
    pub auth: Option<HeaderValue>,
    pub header: &'static str,
    pub url: Url,
    pub chain: u64,
}
pub(super) struct Ticket {
    owner: Address,
    input: Address,
    output: Address,
    amount: U256,
    minimum: U256,
    fee: Option<u32>,
    target: Address,
    runtime: B256,
    generation: u64,
}
fn bad(ctx: &Ctx, reason: &str) -> GatewayError {
    ctx.err(ErrorCode::BadResponse, reason)
}
impl Ticket {
    pub(super) fn new(
        ctx: &Ctx,
        source: &str,
        r: &Request,
        t: &FirmTerms,
        amount: U256,
    ) -> GatewayResult<Self> {
        let generation = ctx.settings.generation();
        let owner = wallet::signer::address(&ctx.with_account(&t.wallet))?;
        if owner.is_zero() || r.trader != Some(owner) {
            return Err(bad(ctx, "Плательщик отличается от выбранного кошелька"));
        }
        let minimum = num::exact_amount(Some(&t.minimum_raw), None, 0)
            .ok_or_else(|| bad(ctx, "Укажите минимальную чистую выплату"))?;
        let key = if source == "kipseli" {
            "kipseli.firm_target"
        } else {
            "zeroex.firm_target"
        };
        let setting = ctx.text(key)?;
        let mut fields = setting.split_whitespace();
        let target = fields
            .next()
            .and_then(|s| s.parse::<Address>().ok())
            .filter(|a| !a.is_zero())
            .ok_or_else(|| bad(ctx, "Некорректный контракт заявки"))?;
        let runtime = fields
            .next()
            .and_then(|s| s.parse::<B256>().ok())
            .filter(|h| !h.is_zero() && *h != keccak256([]))
            .ok_or_else(|| bad(ctx, "Некорректный хеш кода заявки"))?;
        if fields.next().is_some()
            || target == owner
            || target == r.input
            || target == r.output
            || (source == "kipseli") != t.fee_tenths_bps.is_some()
            || t.fee_tenths_bps.is_some_and(|n| n >= 100000)
        {
            return Err(bad(
                ctx,
                "Проверьте назначение контракта и согласованную комиссию источника",
            ));
        }
        Ok(Self {
            owner,
            input: r.input,
            output: r.output,
            amount,
            minimum,
            fee: t.fee_tenths_bps,
            target,
            runtime,
            generation,
        })
    }
    pub(super) async fn preview(&self, ctx: &Ctx, api: RequestApi) -> GatewayResult<Preview> {
        let RequestApi {
            client,
            auth,
            header,
            mut url,
            chain,
        } = api;
        let mut wire = if let Some(fee) = self.fee {
            client.post(url).json(&json!({"chainId":chain,"tokenIn":self.input,"tokenOut":self.output,
                "amountIn":self.amount.to_string(),"minAmountOut":self.minimum.to_string(),"fee":fee.to_string(),"userAddress":self.owner}))
        } else {
            url.query_pairs_mut().extend_pairs([
                ("chainId", chain.to_string()),
                ("taker", self.owner.to_string()),
                ("sellToken", self.input.to_string()),
                ("buyToken", self.output.to_string()),
                ("sellAmount", self.amount.to_string()),
            ]);
            client.get(url).header("0x-version", "v2")
        };
        if let Some(auth) = auth {
            wire = wire.header(header, auth);
        }
        let (status, body) = http::exchange(wire, ctx.limits()?)
            .await
            .map_err(|_| bad(ctx, "Нет полного ответа сервиса индивидуальных котировок"))?;
        if !status.is_success() {
            return Err(bad(
                ctx,
                &format!(
                    "Сервис индивидуальных котировок вернул HTTP {}",
                    status.as_u16()
                ),
            ));
        }
        let data: Value = serde_json::from_str(&body)
            .map_err(|_| bad(ctx, "Некорректный ответ сервиса индивидуальных котировок"))?;
        let (calldata, mut details) = if self.fee.is_some() {
            wire::kipseli(self, &data)
        } else {
            wire::zeroex(self, &data)
        }
        .ok_or_else(|| {
            bad(
                ctx,
                "Разобранные условия предложения отличаются от запроса кошелька",
            )
        })?;
        let head = white_state::head(ctx, "latest").await?;
        let code: Bytes =
            rpc::call(ctx, "eth_getCode", json!([self.target, head.selector()])).await?;
        if keccak256(code) != self.runtime || self.generation != ctx.settings.generation() {
            return Err(bad(ctx, "Изменился контракт или профиль получателя заявки"));
        }
        let read = |token| Call {
            from: self.owner,
            to: token,
            input: IERC20::balanceOfCall { owner: self.owner }
                .abi_encode()
                .into(),
        };
        let swap = Call {
            from: self.owner,
            to: self.target,
            input: calldata,
        };
        let path = if self.fee.is_some() {
            Some(cash::read(ctx, self.target, &head.selector()).await?)
        } else {
            let settler = wire::settler(self, &swap.input)
                .ok_or_else(|| bad(ctx, "Неизвестный адрес Settler"))?;
            registry::verify(ctx, settler, &head.selector()).await?;
            None
        };
        let calls = [
            read(self.input),
            read(self.output),
            swap,
            read(self.input),
            read(self.output),
        ];
        let rows = simulate::run_anchored(
            ctx,
            &calls,
            &[],
            &[],
            head.number.to::<u64>().into(),
            Some(head.hash),
        )
        .await?;
        let [a, b, op, c, d] = rows.as_slice() else {
            return Err(bad(ctx, "Неполная последовательность проверки выплаты"));
        };
        if rows.iter().any(|r| r.status != U64::from(1)) {
            return Err(ctx.err(
                ErrorCode::NoRoute,
                "Проверка своей заявки откатилась при текущих остатках, разрешениях и цене",
            ));
        }
        let val = |x: &simulate::CallResult| {
            (x.return_data.len() == 32).then(|| U256::from_be_slice(&x.return_data))
        };
        let measured = (|| Some((val(a)?.checked_sub(val(c)?)?, val(d)?.checked_sub(val(b)?)?)))();
        let Some((paid, received)) = measured else {
            return Err(bad(ctx, "Нет четырёх корректных балансов кошелька"));
        };
        if paid != self.amount
            || received < self.minimum
            || (self.fee.is_some() && val(op) != Some(received))
            || self.generation != ctx.settings.generation()
        {
            return Err(bad(
                ctx,
                "Расход, выплата или профиль изменились при проверке",
            ));
        }
        if let Some(path) = path
            && !cash::paid(self, path, &op.logs, received)
        {
            return Err(bad(ctx, "Не подтверждены касса и путь оплаты Kipseli"));
        }
        details["type"] = json!("wallet_simulation");
        details["gas_used"] = json!(op.gas_used.to::<u64>());
        details["owner_minimum_raw"] = json!(self.minimum.to_string());
        Ok(Preview {
            source: if self.fee.is_some() {
                "kipseli"
            } else {
                "zeroex"
            }
            .into(),
            input: self.input,
            output: self.output,
            spent_raw: paid.to_string(),
            received_raw: received.to_string(),
            unspent_raw: "0".into(),
            fee: None,
            block_hash: format!("{:#x}", head.hash),
            block: Some(head.number.to()),
            expires: None,
            details,
        })
    }
}
