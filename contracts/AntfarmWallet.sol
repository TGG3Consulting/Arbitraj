// SPDX-License-Identifier: MIT
// Владелец развёртывает модуль в Ethereum. Плательщик задаёт предел расхода входной валюты и отдельной комиссии ATF.
pragma solidity 0.8.37;

interface IAntfarmAsset {
    function balanceOf(address who) external view returns (uint256);
    function allowance(address owner,address spender) external view returns (uint256);
    function approve(address spender,uint256 amount) external returns (bool);
    function transfer(address to,uint256 amount) external returns (bool);
    function transferFrom(address from,address to,uint256 amount) external returns (bool);
}

interface IAntfarmVenue {
    struct Swap {
        uint256 quantity;
        uint256 limit;
        uint256 maxFee;
        address[] path;
        uint16[] fees;
        address to;
        uint256 deadline;
    }
    function factory() external view returns (address);
    function antfarmToken() external view returns (address);
    function swapExactTokensForTokens(Swap calldata s) external returns (uint256[] memory);
    function swapExactTokensForTokensSupportingFeeOnTransferTokens(Swap calldata s) external;
    function swapTokensForExactTokens(Swap calldata s) external returns (uint256[] memory);
}

contract AntfarmWallet {
    IAntfarmVenue public constant router = IAntfarmVenue(0x6D9f0eb21D77C6d24bE49a579508471E937D5418);
    address public constant feeToken = 0x518b63Da813D46556FEa041A88b52e3CAa8C16a8;
    bool transient private entered;

    struct Order {
        address input;
        address output;
        uint16 tier;
        uint16 fundingTier;
        bool supporting;
        uint256 routerQuantity;
        uint256 maxDebit;
        uint256 minimum;
        uint256 maxRouterFee;
        uint256 feeFunding;
        uint256 maxFundingDebit;
        uint256 deadline;
    }
    event Settled(address indexed payer,address indexed input,address indexed output,uint256 debit,uint256 credit,
                  uint256 separateFeeDebit,uint256 separateFeeCredit);

    function tokenCall(address token,bytes memory data) private {
        (bool ok,bytes memory value)=token.call(data);
        require(token.code.length!=0 && ok && (value.length==0 || abi.decode(value,(bool))),"token");
    }
    function approve(address token,uint256 amount) private {
        if (IAntfarmAsset(token).allowance(address(this),address(router))!=0)
            tokenCall(token,abi.encodeCall(IAntfarmAsset.approve,(address(router),0)));
        if (amount!=0) tokenCall(token,abi.encodeCall(IAntfarmAsset.approve,(address(router),amount)));
    }
    function pull(address token,uint256 amount) private {
        if(amount!=0) tokenCall(token,abi.encodeCall(IAntfarmAsset.transferFrom,(msg.sender,address(this),amount)));
    }
    function settleBalance(address token,uint256 opening) private {
        approve(token,0);
        uint256 rest=IAntfarmAsset(token).balanceOf(address(this))-opening;
        if(rest!=0) tokenCall(token,abi.encodeCall(IAntfarmAsset.transfer,(msg.sender,rest)));
        require(IAntfarmAsset(token).balanceOf(address(this))==opening,"module balance");
    }
    function path(address a,address b) private pure returns(address[] memory p) {
        p=new address[](2); p[0]=a; p[1]=b;
    }
    function fees(uint16 tier) private pure returns(uint16[] memory f) {
        f=new uint16[](1); f[0]=tier;
    }
    function execute(Order calldata o) external returns(uint256 debit,uint256 credit) {
        require(!entered && block.timestamp<=o.deadline && o.routerQuantity!=0 && o.maxDebit!=0 &&
                o.minimum!=0 && o.input!=o.output && o.input!=address(0) && o.output!=address(0),"order");
        require(block.chainid==1 && address(router).codehash==0xab093829729d83c3859f76c3343b65622c0cd740c2b40a661bc2e0b5f6a45cb4 &&
                router.antfarmToken()==feeToken,"venue");
        entered=true;
        bool separate=o.input!=feeToken && o.output!=feeToken;
        require(separate || (o.feeFunding==0 && o.fundingTier==0),"fee currency");
        require(o.fundingTier==0 ? o.maxFundingDebit==0 : o.feeFunding!=0 && o.maxFundingDebit<=o.maxDebit,"fee funding");
        uint256 oldIn=IAntfarmAsset(o.input).balanceOf(msg.sender);
        uint256 oldOut=IAntfarmAsset(o.output).balanceOf(msg.sender);
        uint256 oldFee=separate?IAntfarmAsset(feeToken).balanceOf(msg.sender):0;
        uint256 baseIn=IAntfarmAsset(o.input).balanceOf(address(this));
        uint256 baseFee=separate?IAntfarmAsset(feeToken).balanceOf(address(this)):0;
        pull(o.input,o.maxDebit);
        approve(o.input,IAntfarmAsset(o.input).balanceOf(address(this))-baseIn);
        if(separate) {
            if(o.fundingTier==0) {
                pull(feeToken,o.feeFunding);
            } else {
                // Часть ограниченной суммы входа оплачивает точное количество ATF. Комиссия самой пары ATF
                // входит в цену покупки. Предел этой комиссии определяется тарифом и точным выходом.
                uint256 fundingFee=o.feeFunding*o.fundingTier/(1000-o.fundingTier);
                router.swapTokensForExactTokens(IAntfarmVenue.Swap(o.feeFunding,o.maxFundingDebit,fundingFee,
                    path(o.input,feeToken),fees(o.fundingTier),address(this),o.deadline));
            }
            approve(feeToken,IAntfarmAsset(feeToken).balanceOf(address(this))-baseFee);
        }
        IAntfarmVenue.Swap memory trade=IAntfarmVenue.Swap(o.routerQuantity,o.minimum,o.maxRouterFee,
            path(o.input,o.output),fees(o.tier),msg.sender,o.deadline);
        if(o.supporting) router.swapExactTokensForTokensSupportingFeeOnTransferTokens(trade);
        else router.swapExactTokensForTokens(trade);
        settleBalance(o.input,baseIn);
        if(separate) settleBalance(feeToken,baseFee);
        debit=oldIn-IAntfarmAsset(o.input).balanceOf(msg.sender);
        credit=IAntfarmAsset(o.output).balanceOf(msg.sender)-oldOut;
        uint256 finalFee=separate?IAntfarmAsset(feeToken).balanceOf(msg.sender):0;
        uint256 feeDebit=oldFee>finalFee?oldFee-finalFee:0;
        require(debit!=0 && debit<=o.maxDebit && credit>=o.minimum &&
                feeDebit<=(o.fundingTier==0?o.feeFunding:0),"payer limits");
        entered=false;
        emit Settled(msg.sender,o.input,o.output,debit,credit,feeDebit,finalFee>oldFee?finalFee-oldFee:0);
    }
}
