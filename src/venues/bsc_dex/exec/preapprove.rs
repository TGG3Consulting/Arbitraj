//! Файл: venues/bsc_dex/exec/preapprove.rs
//! Что делает: решение 476 (Тигран 20.11: «approve заранее — да, газ — копейки»; меняет решение 303 «заранее не
//! отправляется»; живой NUM 20.11 — первая продажа Gate ждала approve, его блок и голову опроса, «котировке 2525 мс»,
//! затем откат) — разрешения (approve) продажи отправляются ЗАРАНЕЕ, пока монета идёт на кошелёк сделки. План продажи
//! собирается ТЕМ ЖЕ путём, что настоящая продажа и симуляция до покупки (exec/dry_build.rs, режим AsTrade: прямой,
//! умный путь, исполнитель при exec.executor_address), кошельком счёта сделки, на количество и маршрут запроса — и
//! разрешения, которых этому плану не хватает СЕЙЧАС (wallet/approve.rs lacking: сумма меньше продажи или истёк срок
//! Permit2), уходят теми же транзакциями, что перед обменом (exec/swap_send.rs send_approvals: квитанция своим
//! узлом, ожидание головы опроса ≥ блока approve — решение 303). Получатель разрешения — тот, кто заберёт монету по
//! плану (Universal Router / Permit2 / исполнитель — как решит сборка), по факту кода сборки, не по догадке.
//! Бандл (Built::Many) — разрешения и так в одном атомарном бандле с обменом: ничего не отправляется. Нативная BNB —
//! разрешений нет. Монеты на кошельке ещё может не быть — для разрешения она не нужна.
//! Продажу это ни к чему не обязывает: при приходе ядро считает всё как раньше; другой путь / получатель к моменту
//! продажи — нужные разрешения уйдут перед обменом, как раньше.
//! Кто вызывает: venue.rs (контракт dex_preapprove).
//! От чего зависит: exec/{dry_build, swap_send, native}.rs, executor/dry.rs (DryMode, ExecTx), wallet/{signer, nonce}.rs,
//! ctx.rs.
//! Настройки: нет своих (через сборку — exec.dex_slippage_pct, exec.executor_address, dex.*, smart.*).

use rust_decimal::Decimal;

use crate::core::types::{Side, TradeRequest};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::quote::Quoting;
use super::super::wallet::nonce::Nonces;
use super::super::wallet::signer;
use super::dry_build::{self, Built};
use super::executor::dry::DryMode;
use super::native;
use super::txs::Env;

/// Что делает: разрешения продажи `req` заранее (шапка). Принимает: котировку (вид на счёт сделки), окружение
/// отправки, nonce кошелька, запрос продажи (как у исполнения: количество — ожидаемый приход, маршрут плана).
/// Возвращает: газ отправленных разрешений, BNB (None — отправлять было нечего). Ошибки: NotConfigured (счёт не
/// выбран), NoRoute (нет маршрута / сборка), отправка разрешения (с газом уже отправленных — approve_gas_bnb), узел.
pub async fn run(
    q: &Quoting,
    env: &Env,
    nonces: &Nonces,
    req: TradeRequest,
) -> GatewayResult<Option<Decimal>> {
    let ctx = &q.ctx;
    if req.side != Side::Sell {
        return Err(ctx.err(ErrorCode::Internal, "разрешения заранее — только продажа"));
    }
    // Решение 132: только кошельком счёта сделки.
    ctx.bound()?;
    if native::is_native(ctx, &req.asset) {
        return Ok(None);
    }
    let wallet = signer::address(ctx)?;
    let route = req
        .route
        .clone()
        .ok_or_else(|| ctx.err(ErrorCode::NoRoute, "в плане продажи нет маршрута DEX"))?;
    let ready = dry_build::build(q, &req, &route, wallet, DryMode::AsTrade).await?;
    let approvals = match &ready.built {
        Built::One(p) => p.approvals.clone(),
        // Режим AsTrade — развёрнутый исполнитель (подмены кода нет).
        Built::Exec(x) if x.over.is_none() => x.approvals.clone(),
        Built::Exec(_) | Built::Many(_) => return Ok(None),
    };
    if approvals.is_empty() {
        return Ok(None);
    }
    let slot = nonces.acquire(ctx).await?;
    super::swap_send::send_approvals(env, &slot, &approvals).await
}
