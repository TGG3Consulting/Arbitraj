//! Поручение Ryze Base. Сигнатура и EIP‑712 сверены с MultiHopRouter.executeSwapIntents.
//! https://documentation.ryze.pro/abi/amm/MultiHopRouter.json
use alloy::primitives::{Address, B256, Signature, U256};
use alloy::sol_types::{SolCall, SolStruct, eip712_domain};
use serde_json::{Value, json};

alloy::sol! {
    #[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq, Eq)]
    struct Hop { address pool; address tokenIn; address tokenOut; }

    #[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq, Eq)]
    struct SwapIntent {
        address user; address tokenIn; address tokenOut; uint256 amountIn; uint256 minAmountOut;
        Hop[] path; address recipient; uint256 deadline; uint256 nonce;
    }

    struct SignedSwap { SwapIntent intent; bytes signature; }
    struct Price {address token; uint256 priceInWad; uint256 timestamp; uint8 v; bytes32 r; bytes32 s;}
    function executeSwapIntents(SignedSwap[] signedIntents, bytes[] pythUpdateData, Price[] cexPriceData);
    function swapNonces(address user) external view returns(uint256);
    function validatePath(Hop[] path) external view returns(bool);
    function intentFee() external view returns(uint256);
    event IntentSwapExecuted(address indexed user,address indexed tokenIn,address indexed tokenOut,
        uint256 amountIn,uint256 amountOut,address recipient,uint256 nonce,uint256 fee);
}

impl SwapIntent {
    pub fn digest(&self, chain: u64, router: Address) -> B256 {
        self.eip712_signing_hash(&eip712_domain! {
            name: "MultiHopRouter", version: "1", chain_id: chain, verifying_contract: router,
        })
    }

    /// Бот подписывает только вывод себе по согласованной последовательности настоящих однонаправленных рынков.
    pub fn matches(
        &self,
        wallet: Address,
        markets: &[(Address, Address, Address)],
        now: u64,
    ) -> bool {
        if wallet.is_zero()
            || self.user != wallet
            || self.recipient != wallet
            || self.tokenIn == self.tokenOut
            || self.amountIn.is_zero()
            || self.minAmountOut.is_zero()
            || self.deadline < U256::from(now)
            || !(1..=3).contains(&self.path.len())
        {
            return false;
        }
        let mut token = self.tokenIn;
        for (i, hop) in self.path.iter().enumerate() {
            if token != hop.tokenIn
                || token.is_zero()
                || hop.tokenOut.is_zero()
                || hop.pool.is_zero()
                || token == hop.tokenOut
                || self.path[..i]
                    .iter()
                    .any(|prev| prev.pool == hop.pool || prev.tokenIn == hop.tokenOut)
                || !markets.iter().any(|&(p, a, b)| {
                    p == hop.pool
                        && ((token, hop.tokenOut) == (a, b) || (token, hop.tokenOut) == (b, a))
                })
            {
                return false;
            }
            token = hop.tokenOut;
        }
        token == self.tokenOut
    }

    /// Тело POST создаётся после проверки локальной подписи; в журнал или устойчивую запись оно не попадает.
    pub fn submission(&self, router: Address, signature: &Signature) -> Option<Value> {
        if signature
            .recover_address_from_prehash(&self.digest(8453, router))
            .ok()?
            != self.user
        {
            return None;
        }
        let deadline = u64::try_from(self.deadline).ok()?;
        let nonce = u64::try_from(self.nonce).ok()?;
        Some(
            json!({"type":"swap","user":self.user,"recipient":self.recipient,"tokenIn":self.tokenIn,
            "tokenOut":self.tokenOut,"amountIn":self.amountIn.to_string(),"minAmountOut":self.minAmountOut.to_string(),
            "path":self.path,"deadline":deadline,"nonce":nonce,"signature":signature.to_string()}),
        )
    }

    /// Batch relayer может содержать несколько чужих заявок. Требуется ровно одна подлинная подпись нашего поручения.
    pub fn present_in(&self, router: Address, input: &[u8]) -> bool {
        let Ok(batch) = executeSwapIntentsCall::abi_decode_validate(input) else {
            return false;
        };
        let digest = self.digest(8453, router);
        let mut matching = batch
            .signedIntents
            .iter()
            .filter(|item| item.intent == *self);
        let Some(item) = matching.next() else {
            return false;
        };
        matching.next().is_none()
            && Signature::try_from(item.signature.as_ref())
                .ok()
                .and_then(|sig| sig.recover_address_from_prehash(&digest).ok())
                == Some(self.user)
    }
}
