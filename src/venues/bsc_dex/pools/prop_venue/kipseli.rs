//! Проверяет всю цепочку кассы Kipseli по состоянию одного блока Ethereum.
use super::{Roles, bad, batch};
use crate::{gateway::errors::GatewayResult, venues::bsc_dex::ctx::Ctx};
use serde_json::Value;

alloy::sol! {
    interface IKipseliCash {
        function propAMM() external view returns(address);
        function quoteWrapper() external view returns(address);
        function swapImpl() external view returns(address);
        function wallet() external view returns(address);
        function masterCopy() external view returns(address);
    }
}
pub(super) async fn state(ctx: &Ctx, r: &Roles, at: &Value) -> GatewayResult<()> {
    let Some([pool, logic, singleton]) = r.intermediaries else {
        return Ok(());
    };
    let roles = [
        r.router.address,
        r.venue.address,
        pool.address,
        logic.address,
        r.vault.address,
        singleton.address,
        r.fallback.address,
    ];
    if roles
        .iter()
        .enumerate()
        .any(|(i, a)| roles[..i].contains(a))
    {
        return Err(bad(ctx));
    }
    let [p, q, imp, cash, source_cash, master] = [
        batch::view(at, r.venue.address, IKipseliCash::propAMMCall {}),
        batch::view(at, r.venue.address, IKipseliCash::quoteWrapperCall {}),
        batch::view(at, pool.address, IKipseliCash::swapImplCall {}),
        batch::view(at, pool.address, IKipseliCash::walletCall {}),
        batch::view(at, logic.address, IKipseliCash::walletCall {}),
        batch::view(at, r.vault.address, IKipseliCash::masterCopyCall {}),
    ];
    let (v, (), (), ()) = tokio::try_join!(
        batch::read(ctx, [p, q, imp, cash, source_cash, master]),
        pool.check(ctx, at),
        logic.check(ctx, at),
        singleton.check(ctx, at)
    )?;
    if batch::decode(&v[0], IKipseliCash::propAMMCall {})? != pool.address
        || batch::decode(&v[1], IKipseliCash::quoteWrapperCall {})? != logic.address
        || batch::decode(&v[2], IKipseliCash::swapImplCall {})? != logic.address
        || batch::decode(&v[3], IKipseliCash::walletCall {})? != r.vault.address
        || batch::decode(&v[4], IKipseliCash::walletCall {})? != r.vault.address
        || batch::decode(&v[5], IKipseliCash::masterCopyCall {})? != singleton.address
    {
        return Err(bad(ctx));
    }
    Ok(())
}
