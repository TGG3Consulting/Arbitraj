//! Проверка полного ABI и EIP-712 подписанного хеша до обращения к платёжной подписи.
use super::{Request, Roles, Terms, abi};
use abi::{DutchWire, ExclusiveDutchOrder, OrderInfo, TokenPermissions, V3DutchOrder, V3Wire};
use alloy::sol_types::{SolStruct, SolValue, eip712_domain};
use alloy_primitives::U256;
impl Roles {
    fn check(&self, info: &OrderInfo, r: &Request) -> Option<u64> {
        let deadline = u64::try_from(info.deadline).ok()?;
        (self.chain != 0
            && !self.permit.is_zero()
            && !self.reactor.is_zero()
            && self.reactor == info.reactor
            && !r.owner.is_zero()
            && r.owner == info.swapper
            && !r.input.is_zero()
            && r.input != r.output
            && r.amount != U256::ZERO
            && r.minimum != U256::ZERO
            && deadline >= r.now
            && info.additionalValidationContract.is_zero()
            && info.additionalValidationData.is_empty())
        .then_some(deadline)
    }
    /// Тип сообщения задаёт официальный источник. Декодируется и пересобирается весь байтовый аргумент.
    /// Для V3 orderHash не включает поздние cosignerData; отправляется только собственный digest.
    pub fn terms(&self, version: &str, encoded: &[u8], request: &Request) -> Option<Terms> {
        if encoded.len() > 16_384 {
            return None;
        }
        let r = request;
        let domain =
            eip712_domain! {name: "Permit2",chain_id:self.chain,verifying_contract:self.permit,};
        match version {
            "v1" => {
                let o = DutchWire::abi_decode_validate(encoded).ok()?;
                if o.abi_encode() != encoded {
                    return None;
                }
                let deadline = self.check(&o.info, r)?;
                let [out] = o.outputs.as_slice() else {
                    return None;
                };
                if o.input.token != r.input
                    || o.input.startAmount.is_zero()
                    || o.input.endAmount < o.input.startAmount
                    || o.input.endAmount > r.amount
                    || (!r.exact_output
                        && (o.input.startAmount != r.amount || o.input.endAmount != r.amount))
                    || (r.exact_output && out.startAmount != out.endAmount)
                    || out.recipient != r.owner
                    || out.token != r.output
                    || o.decayStartTime > o.decayEndTime
                    || o.decayEndTime > o.info.deadline
                {
                    return None;
                }
                let min_output = out.startAmount.min(out.endAmount);
                if min_output < r.minimum {
                    return None;
                }
                let quoted_output = out.startAmount;
                let nonce = o.info.nonce;
                let (quoted_input, maximum_input) = (o.input.startAmount, o.input.endAmount);
                let witness = ExclusiveDutchOrder {
                    info: o.info,
                    decayStartTime: o.decayStartTime,
                    decayEndTime: o.decayEndTime,
                    exclusiveFiller: o.exclusiveFiller,
                    exclusivityOverrideBps: o.exclusivityOverrideBps,
                    inputToken: o.input.token,
                    inputStartAmount: o.input.startAmount,
                    inputEndAmount: o.input.endAmount,
                    outputs: o.outputs,
                };
                let order_hash = witness.eip712_hash_struct();
                let permit = abi::v1::PermitWitnessTransferFrom {
                    permitted: TokenPermissions {
                        token: r.input,
                        amount: maximum_input,
                    },
                    spender: self.reactor,
                    nonce,
                    deadline: U256::from(deadline),
                    witness,
                };
                Some(Terms {
                    roles: *self,
                    request: *r,
                    order_hash,
                    digest: permit.eip712_signing_hash(&domain),
                    nonce,
                    deadline,
                    min_output,
                    quoted_output,
                    maximum_input,
                    quoted_input,
                })
            }
            "v3" => {
                let o = V3Wire::abi_decode_validate(encoded).ok()?;
                if o.abi_encode() != encoded {
                    return None;
                }
                let deadline = self.check(&o.info, r)?;
                let [out] = o.baseOutputs.as_slice() else {
                    return None;
                };
                if o.baseInput.token != r.input
                    || o.baseInput.startAmount.is_zero()
                    || o.baseInput.maxAmount < o.baseInput.startAmount
                    || o.baseInput.maxAmount > r.amount
                    || (!r.exact_output
                        && (o.baseInput.startAmount != r.amount
                            || o.baseInput.maxAmount != r.amount))
                    || o.cosigner.is_zero()
                    || out.recipient != r.owner
                    || out.token != r.output
                    || out.minAmount < r.minimum
                    || out.startAmount < out.minAmount
                {
                    return None;
                }
                let (min_output, quoted_output, nonce) =
                    (out.minAmount, out.startAmount, o.info.nonce);
                let (quoted_input, maximum_input) =
                    (o.baseInput.startAmount, o.baseInput.maxAmount);
                let witness = V3DutchOrder {
                    info: o.info,
                    cosigner: o.cosigner,
                    startingBaseFee: o.startingBaseFee,
                    baseInput: o.baseInput,
                    baseOutputs: o.baseOutputs,
                };
                let order_hash = witness.eip712_hash_struct();
                let permit = abi::v3::PermitWitnessTransferFrom {
                    permitted: TokenPermissions {
                        token: r.input,
                        amount: maximum_input,
                    },
                    spender: self.reactor,
                    nonce,
                    deadline: U256::from(deadline),
                    witness,
                };
                Some(Terms {
                    roles: *self,
                    request: *r,
                    order_hash,
                    digest: permit.eip712_signing_hash(&domain),
                    nonce,
                    deadline,
                    min_output,
                    quoted_output,
                    maximum_input,
                    quoted_input,
                })
            }
            _ => None,
        }
    }
}
