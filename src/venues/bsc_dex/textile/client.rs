//! HTTP Textile FX v2. Только явный запрос firm с выбранным кошельком резервирует объём у maker.
use super::{profile::Profile, wire};
use crate::{
    gateway::{
        errors::{ErrorCode, GatewayError, GatewayResult},
        http,
    },
    venues::bsc_dex::{ctx::Ctx, wallet},
};
use alloy::{signers::SignerSync, sol_types::SolStruct};
use alloy_primitives::{Address, B256, U256};
use reqwest::{Client, Url, header::HeaderValue};
use serde_json::{Value, json};

alloy::sol! {struct TakerControl {address taker; uint256 chainId; bytes32 nonce; uint256 issuedAt;}}
pub(super) fn fail(ctx: &Ctx, message: &str) -> GatewayError {
    ctx.err(ErrorCode::BadResponse, message)
}
pub(super) struct Query {
    pub input: Address,
    pub output: Address,
    pub amount: U256,
    pub exact_out: bool,
}
impl Query {
    fn body(&self, chain: u64) -> Value {
        let mut b = json!({"chainId":chain,"sellToken":self.input,"buyToken":self.output});
        b[if self.exact_out {
            "buyAmount"
        } else {
            "sellAmount"
        }] = json!(self.amount.to_string());
        b
    }
}
async fn post(ctx: &Ctx, firm: bool, q: &Query, profile: Option<&Profile>) -> GatewayResult<Value> {
    let chain = ctx
        .chain_id()
        .ok_or_else(|| fail(ctx, "Для Textile требуется chainId"))?;
    let base = ctx.text("textile.base_url")?;
    let mut url = Url::parse(&base).map_err(|_| fail(ctx, "Некорректный URL Textile"))?;
    if url.scheme() != "https"
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(fail(
            ctx,
            "Для Textile требуется HTTPS URL без параметров и пароля",
        ));
    }
    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .build()
        .map_err(|_| fail(ctx, "Не удалось открыть HTTPS Textile"))?;
    let mut body = q.body(chain);
    let mut auth = None;
    if let Some(p) = profile {
        if url.as_str().trim_end_matches('/') != "https://api.textilecredit.com/v2" {
            return Err(fail(
                ctx,
                "Для индивидуальной заявки требуется официальный API Textile",
            ));
        }
        body["taker"] = json!(p.taker);
        if ctx.flag("textile.partner").unwrap_or(false) {
            let key = ctx.vault.get("textile.api_key").map_err(|e| ctx.tag(e))?;
            if key.trim().is_empty() {
                return Err(fail(ctx, "Укажите ключ партнёра Textile"));
            }
            let mut value = HeaderValue::from_str(&format!("Bearer {}", key.trim()))
                .map_err(|_| fail(ctx, "Некорректный ключ Textile"))?;
            value.set_sensitive(true);
            auth = Some(value);
        } else {
            if p.protected {
                return Err(fail(
                    ctx,
                    "Для taker-модуля без заранее внесённого инвентаря нужен доступ Textile rfq:aggregator",
                ));
            }
            let mut nonce = [0; 32];
            getrandom::fill(&mut nonce).map_err(|_| fail(ctx, "Не получен nonce Textile"))?;
            let issued = chrono::Utc::now().timestamp_millis().unsigned_abs();
            let nonce = B256::from(nonce);
            let message = TakerControl {
                taker: p.taker,
                chainId: U256::from(chain),
                nonce,
                issuedAt: U256::from(issued),
            };
            let domain = alloy::sol_types::eip712_domain! {name:"Textile Taker Control",version:"1",chain_id:chain};
            let signer = wallet::signer::signer(ctx)?;
            if signer.address() != p.taker {
                return Err(fail(ctx, "Кошелёк Textile изменился"));
            }
            let sig = signer
                .sign_hash_sync(&message.eip712_signing_hash(&domain))
                .map_err(|_| fail(ctx, "Кошелёк не подтвердил контроль Textile"))?;
            body["takerProof"] =
                json!({"nonce":nonce,"issuedAt":issued,"signature":sig.to_string()});
        }
    }
    url.path_segments_mut()
        .map_err(|_| fail(ctx, "Некорректный путь Textile"))?
        .pop_if_empty()
        .push("rfq")
        .push(if firm { "request" } else { "preview" });
    let mut req = client.post(url).json(&body);
    if let Some(value) = auth {
        req = req.header(reqwest::header::AUTHORIZATION, value);
    }
    let (status, body) = http::exchange(req, ctx.limits()?)
        .await
        .map_err(|_| fail(ctx, "Нет полного ответа Textile. Статус запроса неизвестен"))?;
    if !status.is_success() {
        return Err(fail(
            ctx,
            &format!("Textile вернул HTTP {}", status.as_u16()),
        ));
    }
    let response: Value =
        serde_json::from_str(&body).map_err(|_| fail(ctx, "Некорректный JSON Textile"))?;
    if response["data"]["status"] == "no_quote" {
        return Err(ctx.err(
            ErrorCode::NoRoute,
            "Textile не предоставил объём для этой пары",
        ));
    }
    Ok(response)
}
pub(super) async fn preview(ctx: &Ctx, q: &Query) -> GatewayResult<(U256, U256, U256)> {
    let r = post(ctx, false, q, None).await?;
    let d = &r["data"];
    if d["status"] != "preview" {
        return Err(fail(ctx, "Ответ Textile не является preview"));
    }
    wire::amounts(d, q.amount, q.exact_out)
        .ok_or_else(|| fail(ctx, "Суммы preview Textile отличаются от запроса"))
}
pub(super) async fn firm(ctx: &Ctx, q: &Query, p: &Profile) -> GatewayResult<wire::Firm> {
    let v = post(ctx, true, q, Some(p)).await?;
    let d = wire::Demand {
        chain: p.chain,
        reactor: p.reactor,
        taker: p.taker,
        input: q.input,
        output: q.output,
        amount: q.amount,
        exact_out: q.exact_out,
        bps: p.bps,
    };
    wire::immediate(&v, &d, chrono::Utc::now().timestamp().unsigned_abs()).ok_or_else(|| {
        fail(
            ctx,
            "Не подтверждены условия обычной подписанной заявки Textile",
        )
    })
}
