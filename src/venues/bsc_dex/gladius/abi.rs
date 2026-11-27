//! Точный ABI GladiusReactor, публичного Quoter и ограниченной оплаты из кошелька.
#![allow(clippy::too_many_arguments)] // Аргументы генерирует sol! из опубликованных контрактов.
use alloy::sol;
sol! {
    struct Info { address reactor; address swapper; uint256 nonce; uint256 deadline; address validation; bytes validationData; }
    struct Input { address token; uint256 startAmount; uint256 endAmount; }
    struct Output { address token; uint256 startAmount; uint256 endAmount; address recipient; }
    struct Order { Info info; uint256 start; uint256 end; address exclusive; uint256 overrideBps; Input input; Output[] outputs; uint256 threshold; }
    struct ResInput { address token; uint256 amount; uint256 maxAmount; }
    struct ResOutput { address token; uint256 amount; address recipient; }
    struct Resolved { Info info; ResInput input; ResOutput[] outputs; bytes signature; bytes32 orderHash; }
    interface IQuote { function quote(bytes order, bytes sig, uint256 quantity) external returns (Resolved); }
    struct Limit { address input; address output; uint256 minimumDebit; uint256 maximumDebit; uint256 minimumCredit; uint256 deadline; }
    struct Signed { bytes order; bytes sig; }
    interface IWallet {
        function fill(Limit limit, Signed signedOrder, uint256 quantity) external returns(uint256 spent, uint256 received);
        event Settled(address indexed payer, bytes32 indexed orderDigest,address indexed input,address output,uint256 spent,uint256 received);
    }
    interface IReactor { event Fill(bytes32 indexed orderHash,address indexed filler,address indexed swapper,uint256 nonce); }
}
