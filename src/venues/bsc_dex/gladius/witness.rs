//! Хеш GladiusOrder и домен Permit2 проверяются отдельно от ответа публичного Quoter.
use super::{abi::Order, model::Offer, profile::Profile, *};
use alloy::{
    primitives::Signature,
    sol,
    sol_types::{SolCall, SolValue},
};
use alloy_primitives::keccak256;
use serde_json::json;
const INFO: &str = "OrderInfo(address reactor,address swapper,uint256 nonce,uint256 deadline,address additionalValidationContract,bytes additionalValidationData)";
const OUTPUT: &str =
    "DutchOutput(address token,uint256 startAmount,uint256 endAmount,address recipient)";
const AUTH: &str = "TokenPermissions(address token,uint256 amount)";
const ORDER: &str = "GladiusOrder(OrderInfo info,uint256 decayStartTime,uint256 decayEndTime,address exclusiveFiller,uint256 exclusivityOverrideBps,address inputToken,uint256 inputStartAmount,uint256 inputEndAmount,DutchOutput[] outputs,uint256 fillThreshold)";
sol! { interface IValidator { function isValidSignature(bytes32 digest,bytes signature) external view returns(bytes4); } }
fn typehash(parts: &[&str]) -> B256 {
    keccak256(parts.concat().as_bytes())
}
pub(super) fn hash(o: &Order) -> B256 {
    let i = &o.info;
    let info = keccak256(
        (
            typehash(&[INFO]),
            i.reactor,
            i.swapper,
            i.nonce,
            i.deadline,
            i.validation,
            keccak256(&i.validationData),
        )
            .abi_encode(),
    );
    let mut output = Vec::with_capacity(32 * o.outputs.len());
    for d in &o.outputs {
        output.extend_from_slice(
            keccak256(
                (
                    typehash(&[OUTPUT]),
                    d.token,
                    d.startAmount,
                    d.endAmount,
                    d.recipient,
                )
                    .abi_encode(),
            )
            .as_slice(),
        );
    }
    keccak256(
        (
            typehash(&[ORDER, OUTPUT, INFO]),
            info,
            o.start,
            o.end,
            o.exclusive,
            o.overrideBps,
            o.input.token,
            o.input.startAmount,
            o.input.endAmount,
            keccak256(output),
            o.threshold,
        )
            .abi_encode(),
    )
}
pub(super) fn digest(o: &Offer, domain: B256) -> B256 {
    let order = &o.order;
    let i = &order.info;
    let permissions =
        keccak256((typehash(&[AUTH]), order.input.token, order.input.endAmount).abi_encode());
    let witness_type = typehash(&[
        "PermitWitnessTransferFrom(TokenPermissions permitted,address spender,uint256 nonce,uint256 deadline,GladiusOrder witness)",
        OUTPUT,
        ORDER,
        INFO,
        AUTH,
    ]);
    let permit = keccak256(
        (
            witness_type,
            permissions,
            i.reactor,
            i.nonce,
            i.deadline,
            o.api.order_hash,
        )
            .abi_encode(),
    );
    let mut msg = Vec::from([0x19, 0x01]);
    msg.extend_from_slice(domain.as_slice());
    msg.extend_from_slice(permit.as_slice());
    keccak256(msg)
}
pub(super) fn eoa(digest: B256, signature: &[u8]) -> Option<Address> {
    let compact;
    let bytes = if signature.len() == 64 {
        let mut full = signature.to_vec();
        let parity = full[32] >> 7;
        full[32] &= 0x7f;
        full.push(27 + parity);
        compact = full;
        &compact[..]
    } else {
        signature
    };
    Signature::try_from(bytes)
        .ok()?
        .recover_address_from_prehash(&digest)
        .ok()
}
#[derive(Clone, Copy)]
pub(super) struct Domain {
    pub permit: Address,
    pub separator: B256,
}
impl Domain {
    pub(super) fn separator(chain: u64, permit: Address) -> B256 {
        keccak256(
            (
                typehash(&["EIP712Domain(string name,uint256 chainId,address verifyingContract)"]),
                keccak256("Permit2".as_bytes()),
                U256::from(chain),
                permit,
            )
                .abi_encode(),
        )
    }
    pub(super) async fn read(ctx: &Ctx, p: &Profile, h: white_state::Head) -> Option<Self> {
        let word = async |to, method: &str| {
            let raw:Bytes=rpc::call(ctx,"eth_call",json!([{"to":to,"input":Bytes::copy_from_slice(&keccak256(method.as_bytes())[..4])},h.selector()])).await.ok()?;
            (raw.len() == 32).then(|| B256::from_slice(&raw))
        };
        let role = word(p.reactor.address, "permit2()").await?;
        let permit = Address::from_word(role);
        let separator = word(permit, "DOMAIN_SEPARATOR()").await?;
        let expected = Self::separator(p.chain, permit);
        (permit != Address::ZERO && role == permit.into_word() && expected == separator)
            .then_some(Self { permit, separator })
    }
    pub(super) async fn authentic(&self, ctx: &Ctx, o: &Offer, h: white_state::Head) -> bool {
        let d = digest(o, self.separator);
        let maker = o.order.info.swapper;
        let Ok(code) = rpc::call::<Bytes>(ctx, "eth_getCode", json!([maker, h.selector()])).await
        else {
            return false;
        };
        if code.is_empty() {
            return eoa(d, &o.api.signature) == Some(maker);
        }
        // Permit2 выбирает ERC-1271 для любого владельца с кодом, включая EIP-7702.

        let call = IValidator::isValidSignatureCall {
            digest: d,
            signature: o.api.signature.clone(),
        };
        let bytes:GatewayResult<Bytes>=rpc::call(ctx,"eth_call",json!([{"from":self.permit,"to":maker,"input":Bytes::from(call.abi_encode())},h.selector()])).await;
        bytes.is_ok_and(|b| {
            IValidator::isValidSignatureCall::abi_decode_returns_validate(&b)
                .is_ok_and(|v| v.as_slice() == [0x16, 0x26, 0xba, 0x7e])
        })
    }
}
