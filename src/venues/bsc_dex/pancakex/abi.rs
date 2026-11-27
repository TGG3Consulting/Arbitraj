//! Байтовый ABI PCSX SDK 1.7.3 и свидетели Permit2 двух исполненных поколений.
use alloy::sol;
sol! {
    struct OrderInfo { address reactor; address swapper; uint256 nonce; uint256 deadline; address additionalValidationContract; bytes additionalValidationData; }
    struct TokenPermissions { address token; uint256 amount; }
    struct DutchInput { address token; uint256 startAmount; uint256 endAmount; }
    struct DutchOutput { address token; uint256 startAmount; uint256 endAmount; address recipient; }
    struct DutchWire { OrderInfo info; uint256 decayStartTime; uint256 decayEndTime; address exclusiveFiller; uint256 exclusivityOverrideBps; DutchInput input; DutchOutput[] outputs; }
    struct ExclusiveDutchOrder { OrderInfo info; uint256 decayStartTime; uint256 decayEndTime; address exclusiveFiller; uint256 exclusivityOverrideBps; address inputToken; uint256 inputStartAmount; uint256 inputEndAmount; DutchOutput[] outputs; }
    struct NonlinearDutchDecay { uint256 relativeBlocks; int256[] relativeAmounts; }
    struct V3DutchInput { address token; uint256 startAmount; NonlinearDutchDecay curve; uint256 maxAmount; uint256 adjustmentPerGweiBaseFee; }
    struct V3DutchOutput { address token; uint256 startAmount; NonlinearDutchDecay curve; address recipient; uint256 minAmount; uint256 adjustmentPerGweiBaseFee; }
    struct V3DutchOrder { OrderInfo info; address cosigner; uint256 startingBaseFee; V3DutchInput baseInput; V3DutchOutput[] baseOutputs; }
    struct CosignerData { uint256 decayStartBlock; address exclusiveFiller; uint256 exclusivityOverrideBps; uint256 inputAmount; uint256[] outputAmounts; }
    struct V3Wire { OrderInfo info; address cosigner; uint256 startingBaseFee; V3DutchInput baseInput; V3DutchOutput[] baseOutputs; CosignerData cosignerData; bytes cosignature; }
}
pub(super) mod v1 {
    use super::{ExclusiveDutchOrder, TokenPermissions};
    alloy::sol! { struct PermitWitnessTransferFrom {TokenPermissions permitted; address spender; uint256 nonce; uint256 deadline; ExclusiveDutchOrder witness;} }
}
pub(super) mod v3 {
    use super::{TokenPermissions, V3DutchOrder};
    alloy::sol! { struct PermitWitnessTransferFrom {TokenPermissions permitted; address spender; uint256 nonce; uint256 deadline; V3DutchOrder witness;} }
}
