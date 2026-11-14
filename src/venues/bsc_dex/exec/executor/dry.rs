//! Файл: venues/bsc_dex/exec/executor/dry.rs
//! Что делает: сборка «сухой сделки DEX» (решение 121, C4) через исполнитель ArbExecutor (решение 122, часть X):
//! - «как сделка»: графовый маршрут формы «бандл» при заданном exec.executor_address — вызов развёрнутого исполнителя
//!   (разрешения — недостающие, с узла), как пойдёт сделка (exec/trade.rs);
//! - «через исполнитель» (режим проверки ДО развёртывания): ЛЮБОЙ маршрут котировки собирается шагами исполнителя, а в
//!   симуляцию eth_simulateV1 подставляются код исполнителя (contracts/out/ArbExecutor.bin-runtime, хеш сверен), слот
//!   владельца = кошелёк бота и слот обёртки = dex.native_wrapped (ревью 272 А1; stateOverrides) на свободном адресе — том, который исполнитель получит при развёртывании
//!   с кошелька бота сейчас (CREATE: адрес из кошелька и его nonce, кода там нет — проверяется). Разрешение ERC-20 этому
//!   адресу — первым вызовом той же симуляции. Так исполнитель проверяется на живых пулах, не будучи развёрнутым.
//!
//! Решение 272: монета — нативный BNB — вызов executeNative (nat): режим «через исполнитель» подставляет код v2 (есть
//! всегда); «как сделка» — только у исполнителя v2 (onchain.rs). Продажа BNB — без разрешений (value).
//! Кто вызывает: exec/dry_build.rs.
//! От чего зависит: exec/executor/{call, code, deploy, onchain}.rs, check/simulate.rs (Override), wallet/approve.rs, node/rpc.rs.
//! Настройки: exec.executor_address [bsc_dex]; через вызываемые — dex.deadline_s, dex.approve_unlimited.

use alloy_primitives::{Address, B256, Bytes};

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::super::check::simulate::Override;
use super::super::super::ctx::Ctx;
use super::super::super::node::rpc;
use super::super::super::wallet::approve::Approval;
use super::super::super::wallet::approve_executor;
use super::super::chain::Step;
use super::call::{self, ExecCall};
use super::{code, deploy, onchain};

/// Режим сухой сделки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DryMode {
    /// Как пойдёт сделка (бандл / исполнитель по exec.executor_address / Universal Router).
    AsTrade,
    /// Любой маршрут — через исполнитель с подставленным кодом (до развёртывания).
    Executor,
}

/// Собранный вызов исполнителя для симуляции.
pub struct ExecTx {
    pub call: ExecCall,
    /// Разрешения ERC-20 исполнителю (первыми вызовами симуляции).
    pub approvals: Vec<Approval>,
    /// Подмена: код и владелец на свободном адресе (режим Executor); None — развёрнутый исполнитель.
    pub over: Option<Override>,
    /// Форма маршрута для показа.
    pub form: String,
}

/// Собирает вызов исполнителя. Принимает: доли (шаги exec/chain.rs с минимумами), кошелёк, время блока, режим, адрес из
/// exec.executor_address (режим AsTrade), (обёртка, продажа ли) — нативный BNB (решение 272) или None.
/// Ошибки: NoRoute (пул без шага исполнителя, адрес занят кодом, исполнитель v1 при нативном BNB), QuoteMismatch,
/// NotConfigured, Internal (артефакт не сверен по хешу), узел.
pub async fn build(
    ctx: &Ctx,
    legs: &[Vec<Step>],
    wallet: Address,
    now: u64,
    (mode, deployed): (DryMode, Option<Address>),
    nat: Option<(Address, bool)>,
) -> GatewayResult<ExecTx> {
    let native = |c: ExecCall| match nat {
        Some((w, wrap_in)) => c.with_native(ctx, w, wrap_in),
        None => Ok(c),
    };
    let internal = |w: String| ctx.err(ErrorCode::Internal, w);
    match (mode, deployed) {
        (DryMode::AsTrade, Some(x)) => {
            let version = onchain::ensure(ctx, x, wallet).await?;
            if nat.is_some() && version != super::code::Version::V2 {
                let why = format!(
                    "исполнитель {x:#x}: код в сети — версия {}, executeNative нет — нативный BNB не исполняется",
                    version.label()
                );
                return Err(ctx.err(ErrorCode::NoRoute, why));
            }
            // Ревью 272 (А1): обёртка в конструкторе исполнителя ≠ dex.native_wrapped — нативный BNB не исполняется.
            if let (Some(_), Some(why)) = (nat, super::native_block(ctx)) {
                return Err(ctx.err(ErrorCode::NoRoute, why));
            }
            let call = native(call::build(ctx, legs, x, now)?)?;
            let approvals = super::approvals(ctx, &call, wallet, x).await?;
            let form = format!("граф: исполнитель {x:#x}");
            Ok(ExecTx {
                call,
                approvals,
                over: None,
                form,
            })
        }
        (DryMode::AsTrade, None) => Err(internal(format!(
            "у кошелька «{}» не развёрнут исполнитель (экран «Маппинг кошельков»)",
            ctx.wallet_name()
        ))),
        (DryMode::Executor, _) => {
            let nonce = deploy::next_nonce(ctx, wallet).await?;
            let at = wallet.create(nonce);
            let have: Bytes =
                rpc::call(ctx, "eth_getCode", serde_json::json!([at, "latest"])).await?;
            if !have.is_empty() {
                let why = format!(
                    "адрес {at:#x} (кошелёк, nonce {nonce}) уже с кодом — подставить исполнитель некуда"
                );
                return Err(ctx.err(ErrorCode::NoRoute, why));
            }
            // Ревью 272 (А1): слот обёртки (конструктор v2) = dex.native_wrapped — как запишет развёртывание; не задана —
            // слот не подставляется (execute его не читает, executeNative без обёртки здесь не собирается: nat пуст).
            let mut state = vec![(
                code::owner_slot().map_err(internal)?,
                B256::left_padding_from(wallet.as_slice()),
            )];
            if let Ok(w) = ctx.addr("dex.native_wrapped") {
                state.push((
                    code::wnative_slot().map_err(internal)?,
                    B256::left_padding_from(w.as_slice()),
                ));
            }
            let over = Override {
                address: at,
                code: code::runtime().map_err(internal)?,
                state,
            };
            let call = native(call::build(ctx, legs, at, now)?)?;
            let approvals = if call.value().is_zero() {
                approve_executor::all(ctx, call.token_in, at, call.amount_in)?
            } else {
                Vec::new()
            };
            let form = format!(
                "через исполнитель: код подставлен в симуляцию по адресу {at:#x} (nonce {nonce})"
            );
            Ok(ExecTx {
                call,
                approvals,
                over: Some(over),
                form,
            })
        }
    }
}
