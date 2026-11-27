//! Идентификация исполненной ноги Kipseli Base по адресу кассы и трём фактическим переводам входа.
use super::{Ticket, bad};
use crate::{
    gateway::errors::GatewayResult,
    venues::bsc_dex::{
        check::simulate::SimLog,
        ctx::Ctx,
        exec::{fill, signed_call},
        node::{call, rpc},
    },
};
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use serde_json::{Value, json};
alloy::sol! { interface ISource {function swapImpl() external view returns(address);}}
pub(super) async fn read(ctx: &Ctx, router: Address, at: &Value) -> GatewayResult<[Address; 3]> {
    let fail = || bad(ctx, "Изменились участники оплаты Kipseli");
    let config = ctx.text("kipseli.firm_path")?;
    let fields = config.split_whitespace().collect::<Vec<_>>();
    if fields.len() != 6 {
        return Err(fail());
    }
    let mut path = [Address::ZERO; 3];
    let mut hashes = [B256::ZERO; 3];
    for (i, entry) in fields.chunks_exact(2).enumerate() {
        path[i] = entry[0].parse().map_err(|_| fail())?;
        hashes[i] = entry[1].parse().map_err(|_| fail())?;
        if path[i].is_zero()
            || path[i] == router
            || path[..i].contains(&path[i])
            || hashes[i].is_zero()
            || hashes[i] == keccak256([])
        {
            return Err(fail());
        }
    }
    let code = |addr| rpc::call::<Bytes>(ctx, "eth_getCode", json!([addr, at]));
    let (impl_addr, a, b, c) = tokio::try_join!(
        call::canonical(ctx, at, router, ISource::swapImplCall {}),
        code(path[0]),
        code(path[1]),
        code(path[2])
    )?;
    if impl_addr != path[0] || [keccak256(a), keccak256(b), keccak256(c)] != hashes {
        return Err(fail());
    }
    Ok(path)
}
pub(super) fn paid(t: &Ticket, path: [Address; 3], rows: &[SimLog], received: U256) -> bool {
    if path.contains(&t.owner) {
        return false;
    }
    let Some(logs) = signed_call::logs(rows) else {
        return false;
    };
    let [source, forward, cash] = path;
    let moves = |token, from, to| fill::moved_logs(&logs, token, Some(from), Some(to));
    moves(t.input, t.owner, source) == t.amount
        && moves(t.input, source, forward) == t.amount
        && moves(t.input, forward, cash) == t.amount
        && moves(t.output, cash, t.owner) == received
}
