//! Подписанные поля исходной котировки ClipperDirect. Внешний сервер не определяет calldata.
use alloy::{
    primitives::{Address, B256, Bytes, Signature, U256, keccak256},
    sol_types::{SolCall, SolValue},
};
use serde::Deserialize;

alloy::sol! {
    struct Sig { uint8 v; bytes32 r; bytes32 s; }
    function transmitAndSwap(address input,address output,uint256 amount,uint256 received,uint256 until,address recipient,Sig sig,bytes aux) external;
    function sellEthForToken(address output,uint256 amount,uint256 received,uint256 until,address recipient,Sig sig,bytes aux) external payable;
    function transmitAndSellTokenForEth(address input,uint256 amount,uint256 received,uint256 until,address recipient,Sig sig,bytes aux) external;
    function DESIGNATED_SIGNER() external view returns(address);
    function WRAPPER_CONTRACT() external view returns(address);
    function isToken(address t) external view returns(bool);
    function getEthBalance(address owner) external view returns(uint256);
}

#[derive(Deserialize)]
pub(super) struct ApiSignature {
    v: u8,
    r: B256,
    s: B256,
}
#[derive(Deserialize)]
pub(super) struct Quote {
    pub chain_id: u64,
    pub clipper_exchange_address: Address,
    pub input_asset_address: Address,
    pub output_asset_address: Address,
    pub input_amount: String,
    pub output_amount: String,
    pub good_until: String,
    pub destination_address: Address,
    pub signature: ApiSignature,
}
pub(super) struct Offer {
    pub chain: u64,
    pub pool: Address,
    pub owner: Address,
    pub input: Address,
    pub output: Address,
    pub amount: U256,
    pub received: U256,
    pub until: U256,
}
impl Offer {
    pub fn digest(&self) -> B256 {
        let domain=keccak256((keccak256("EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)"),
            keccak256("ClipperDirect"),keccak256("1.0.0"),U256::from(self.chain),self.pool).abi_encode());
        let fields=keccak256((keccak256("OfferStruct(address input_token,address output_token,uint256 input_amount,uint256 output_amount,uint256 good_until,address destination_address)"),
            self.input,self.output,self.amount,self.received,self.until,self.owner).abi_encode());
        keccak256([b"\x19\x01".as_slice(), domain.as_slice(), fields.as_slice()].concat())
    }
    pub fn expires(&self) -> u64 {
        (self.until & U256::from(u32::MAX)).to::<u64>()
    }
    pub fn valid(&self, q: &Quote, signer: Address) -> bool {
        let n = |s: &str| U256::from_str_radix(s, 10).ok();
        if signer.is_zero()
            || q.chain_id != self.chain
            || q.clipper_exchange_address != self.pool
            || q.input_asset_address != self.input
            || q.output_asset_address != self.output
            || q.destination_address != self.owner
            || n(&q.input_amount) != Some(self.amount)
            || n(&q.output_amount) != Some(self.received)
            || n(&q.good_until) != Some(self.until)
            || self.received.is_zero()
            || !matches!(q.signature.v, 27 | 28)
        {
            return false;
        }
        let sig = Signature::new(
            U256::from_be_bytes(q.signature.r.0),
            U256::from_be_bytes(q.signature.s.0),
            q.signature.v == 28,
        );
        let d = self.digest();
        let affiliate = keccak256([d.as_slice(), self.owner.as_slice()].concat());
        [d, affiliate]
            .iter()
            .any(|h| sig.recover_address_from_prehash(h).ok() == Some(signer))
    }
    pub fn data(&self, q: &Quote, native_in: bool, native_out: bool) -> Bytes {
        let sig = Sig {
            v: q.signature.v,
            r: q.signature.r,
            s: q.signature.s,
        };
        let (amount, received, until, recipient, aux) = (
            self.amount,
            self.received,
            self.until,
            self.owner,
            Bytes::new(),
        );
        if native_in {
            sellEthForTokenCall {
                output: self.output,
                amount,
                received,
                until,
                recipient,
                sig,
                aux,
            }
            .abi_encode()
        } else if native_out {
            transmitAndSellTokenForEthCall {
                input: self.input,
                amount,
                received,
                until,
                recipient,
                sig,
                aux,
            }
            .abi_encode()
        } else {
            transmitAndSwapCall {
                input: self.input,
                output: self.output,
                amount,
                received,
                until,
                recipient,
                sig,
                aux,
            }
            .abi_encode()
        }
        .into()
    }
}
