//! Привязка настроенных ролей Antfarm к исполняемому коду на каноническом блоке.
use super::super::ctx::Ctx;
use super::{nami_batch as batch, solidly_classic_factory};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::{sol, sol_types::SolCall};
use alloy_primitives::{Address, B256, keccak256};
use serde_json::{Value, json};

sol! { interface IModule {
    function router() external view returns(address);
    function feeToken() external view returns(address);
    function vault() external view returns(address);
}}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Profile {
    pub factory: Address,
    pub router: Address,
    pub atf: Address,
    pub factory_hash: B256,
    pub router_hash: B256,
    pub runtime: [B256; 2],
    init: [B256; 2],
}
fn invalid(ctx: &Ctx) -> crate::gateway::errors::GatewayError {
    ctx.err(
        ErrorCode::NotConfigured,
        "Antfarm: требуются отдельные подтверждённые роли и хеши выбранного развёртывания",
    )
}
fn parse(s: &str) -> Option<B256> {
    s.parse::<B256>()
        .ok()
        .filter(|h| *h != B256::ZERO && *h != keccak256([]))
}
fn hash(ctx: &Ctx, id: &str, key: &str) -> GatewayResult<B256> {
    parse(&ctx.dex_text(key, id)?).ok_or_else(|| invalid(ctx))
}
fn pair(ctx: &Ctx, id: &str, key: &str) -> GatewayResult<[B256; 2]> {
    let v = ctx.dex_list(key, id)?;
    let [a, b] = v.as_slice() else {
        return Err(invalid(ctx));
    };
    let out = [
        parse(a).ok_or_else(|| invalid(ctx))?,
        parse(b).ok_or_else(|| invalid(ctx))?,
    ];
    (out[0] != out[1])
        .then_some(out)
        .ok_or_else(|| invalid(ctx))
}
impl Profile {
    pub fn read(ctx: &Ctx, id: &str) -> GatewayResult<Self> {
        let factory = ctx.dex_addr("dex.factory", id)?;
        let router = ctx.dex_addr("dex.quoter", id)?;
        let atf = ctx.dex_addr("dex.antfarm_fee_token", id)?;
        if [factory, router, atf].contains(&Address::ZERO)
            || factory == router
            || factory == atf
            || router == atf
        {
            return Err(invalid(ctx));
        }
        Ok(Self {
            factory,
            router,
            atf,
            factory_hash: hash(ctx, id, "dex.factory_runtime_hash")?,
            router_hash: hash(ctx, id, "dex.router_runtime_hash")?,
            init: pair(ctx, id, "dex.pool_code_hashes")?,
            runtime: pair(ctx, id, "dex.antfarm_pair_runtime_hashes")?,
        })
    }
    pub fn derived(&self, tokens: [Address; 2], tier: u16) -> Address {
        let mut salt = [0u8; 62];
        salt[..20].copy_from_slice(tokens[0].as_slice());
        salt[20..40].copy_from_slice(tokens[1].as_slice());
        salt[40..42].copy_from_slice(&tier.to_be_bytes());
        salt[42..].copy_from_slice(self.atf.as_slice());
        self.factory.create2(
            keccak256(salt),
            self.init[usize::from(tokens[0] == self.atf)],
        )
    }
}
pub(in crate::venues::bsc_dex) fn vault(ctx: &Ctx, id: &str) -> GatewayResult<(Address, B256)> {
    let address = ctx.dex_addr("dex.vault", id)?;
    if address.is_zero() {
        return Err(invalid(ctx));
    }
    Ok((address, hash(ctx, id, "dex.antfarm_vault_runtime_hash")?))
}
#[derive(Clone, Copy)]
struct ObservedModule {
    runtime: B256,
    router: Address,
    fee_token: Address,
    vault: Option<Address>,
}
type ModuleKey = (B256, Address, bool);
type ModuleSlot = (
    ModuleKey,
    std::sync::Arc<tokio::sync::OnceCell<ObservedModule>>,
);
static MODULES: std::sync::LazyLock<std::sync::Mutex<std::collections::VecDeque<ModuleSlot>>> =
    std::sync::LazyLock::new(Default::default);

fn module_slot(key: ModuleKey) -> std::sync::Arc<tokio::sync::OnceCell<ObservedModule>> {
    let mut cache = MODULES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, cell)) = cache.iter().find(|(k, _)| *k == key) {
        return cell.clone();
    }
    let cell = std::sync::Arc::new(tokio::sync::OnceCell::new());
    if cache.len() >= 64 {
        cache.pop_front();
    }
    cache.push_back((key, cell.clone()));
    cell
}
async fn observe(
    ctx: &Ctx,
    target: Address,
    tag: &Value,
    output: bool,
) -> GatewayResult<ObservedModule> {
    let roles = batch::read(
        ctx,
        [
            ("eth_getCode", json!([target, tag])),
            batch::view(tag, target, IModule::routerCall {}),
            batch::view(tag, target, IModule::feeTokenCall {}),
        ],
    );
    let lender = async {
        if !output {
            return Ok(None);
        }
        let [raw] = batch::read(ctx, [batch::view(tag, target, IModule::vaultCall {})]).await?;
        batch::decode(&raw, IModule::vaultCall {}).map(Some)
    };
    let ([code, r, t], vault) = tokio::try_join!(roles, lender)?;
    let selector = if output {
        super::super::exec::antfarm_output_wallet::selector()
    } else {
        super::super::exec::antfarm_wallet::selector()
    };
    if !code
        .windows(5)
        .any(|op| op[0] == 0x63 && op[1..] == selector)
    {
        return Err(invalid(ctx));
    }
    Ok(ObservedModule {
        runtime: keccak256(&code),
        router: batch::decode(&r, IModule::routerCall {})?,
        fee_token: batch::decode(&t, IModule::feeTokenCall {})?,
        vault,
    })
}
pub(in crate::venues::bsc_dex) async fn module(
    ctx: &Ctx,
    id: &str,
    tag: &Value,
    output: bool,
) -> GatewayResult<(Address, B256)> {
    let generation = ctx.settings.generation();
    let block = tag["blockHash"]
        .as_str()
        .and_then(|s| s.parse::<B256>().ok())
        .ok_or_else(|| invalid(ctx))?;
    let p = Profile::read(ctx, id)?;
    let key = if output {
        "dex.antfarm_output_wallet"
    } else {
        "dex.router"
    };
    let target = ctx.dex_addr(key, id)?;
    if ctx.chain_id() != Some(1)
        || tag["requireCanonical"] != true
        || [Address::ZERO, p.factory, p.router, p.atf].contains(&target)
    {
        return Err(invalid(ctx));
    }
    let accepted = super::direct_module::configured(ctx, id)?;
    let loan_vault = if output {
        Some(ctx.dex_addr("dex.vault", id)?)
    } else {
        None
    };
    let slot = module_slot((block, target, output));
    let fact = slot
        .get_or_try_init(|| observe(ctx, target, tag, output))
        .await?;
    if !accepted.contains(&fact.runtime)
        || fact.router != p.router
        || fact.fee_token != p.atf
        || fact.vault != loan_vault
        || fact.vault == Some(Address::ZERO)
    {
        return Err(invalid(ctx));
    }
    if generation != ctx.settings.generation() {
        return Err(ctx.err(ErrorCode::StaleData, "Antfarm: изменились настройки модуля"));
    }
    Ok((target, fact.runtime))
}
pub(in crate::venues::bsc_dex) fn receipt_runtime(
    output: bool,
    atf: Option<Address>,
    runtime: Option<B256>,
) -> Option<B256> {
    match (atf, runtime) {
        (Some(t), Some(h)) if !t.is_zero() && h != B256::ZERO && h != keccak256([]) => Some(h),
        (None, None) => legacy_runtime(output),
        _ => None,
    }
}
/// Историческая аттестация применяется только к уже подписанным поручениям старого формата.
fn legacy_runtime(output: bool) -> Option<B256> {
    let roles: [B256; 2] = serde_json::from_str(include_str!(
        "../../../../config/legacy_antfarm_receipt_runtimes.json"
    ))
    .ok()?;
    Some(roles[usize::from(output)])
}
pub(in crate::venues::bsc_dex) async fn inclusion(
    ctx: &Ctx,
    block: Option<B256>,
    target: Address,
    output: bool,
    atf: Option<Address>,
    saved_hash: Option<B256>,
) -> GatewayResult<(Address, B256)> {
    let fail = || {
        ctx.err(
            ErrorCode::OutcomeUnknown,
            "Antfarm: не подтверждены исходный runtime и токен комиссии на блоке квитанции",
        )
    };
    let runtime = receipt_runtime(output, atf, saved_hash).ok_or_else(fail)?;
    let tag = json!({"blockHash":block.ok_or_else(fail)?,"requireCanonical":true});
    let (_, [raw]) = tokio::try_join!(
        solidly_classic_factory::plain(ctx, target, runtime, &tag),
        batch::read(ctx, [batch::view(&tag, target, IModule::feeTokenCall {})])
    )
    .map_err(|_| fail())?;
    let fee_token = IModule::feeTokenCall::abi_decode_returns_validate(&raw).map_err(|_| fail())?;
    if fee_token.is_zero() || atf.is_some_and(|t| t != fee_token) {
        return Err(fail());
    }
    Ok((fee_token, runtime))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    fn archive(bytes: &[u8]) -> Value {
        serde_json::from_reader(flate2::read::GzDecoder::new(bytes)).unwrap()
    }
    #[test]
    fn every_registered_pair_uses_its_own_creation_and_runtime_class() {
        let profiles: Value =
            serde_json::from_str(include_str!("../../../../config/first_run_dex.json")).unwrap();
        let cfg = &profiles
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == "antfarm-ethereum")
            .unwrap()["settings"];
        let data = archive(include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/antfarm_ethereum_registry.json.gz"
        ));
        let addr = |v: &Value| serde_json::from_value::<Address>(v.clone()).unwrap();
        let hash = |v: &Value| serde_json::from_value::<B256>(v.clone()).unwrap();
        let p = Profile {
            factory: addr(&cfg["dex.factory"]),
            router: addr(&cfg["dex.quoter"]),
            atf: addr(&cfg["dex.antfarm_fee_token"]),
            factory_hash: hash(&cfg["dex.factory_runtime_hash"]),
            router_hash: hash(&cfg["dex.router_runtime_hash"]),
            init: std::array::from_fn(|i| hash(&cfg["dex.pool_code_hashes"][i])),
            runtime: std::array::from_fn(|i| hash(&cfg["dex.antfarm_pair_runtime_hashes"][i])),
        };
        assert_eq!(p.atf, addr(&data["fee_token"]));
        assert_eq!(
            p.factory_hash,
            hash(&data["contracts"]["AntfarmFactory"]["hash"])
        );
        assert_eq!(
            p.router_hash,
            hash(&data["contracts"]["AntfarmRouter"]["hash"])
        );
        let mut wrong = p;
        wrong.init.swap(0, 1);
        for row in data["pairs"].as_array().unwrap() {
            let tokens: [Address; 2] = std::array::from_fn(|i| addr(&row["tokens"][i]));
            let tier = row["fee"].as_u64().unwrap() as u16;
            let pool = addr(&row["pair"]);
            assert_eq!(p.derived(tokens, tier), pool);
            assert_ne!(wrong.derived(tokens, tier), pool);
            assert_eq!(
                p.runtime[usize::from(tokens[0] == p.atf)],
                hash(&row["runtime_hash"])
            );
        }
        let stock = archive(include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/antfarm_protected_ethereum.json.gz"
        ));
        let output = archive(include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/antfarm_output_ethereum.json.gz"
        ));
        for (mode, fixture) in [(false, stock), (true, output)] {
            let deployed = hash(&fixture["runtime_hash"]);
            assert_eq!(legacy_runtime(mode), Some(deployed));
            assert_ne!(legacy_runtime(!mode), Some(deployed));
            assert!(
                cfg["dex.module_runtime_hashes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|v| hash(v) == deployed)
            );
            let newer = B256::repeat_byte(8);
            assert_eq!(receipt_runtime(mode, Some(p.atf), Some(newer)), Some(newer));
            assert_eq!(receipt_runtime(mode, None, None), Some(deployed));
        }
    }
}
