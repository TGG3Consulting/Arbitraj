// SPDX-License-Identifier: MIT
// Модуль развёртывает владелец в Ethereum. Плательщик задаёт пределы комиссии и минимальную чистую выплату.
// Комиссия пары Antfarm покупается за валюту выплаты. Максимумы и минимум проверяются в кошельке плательщика.
pragma solidity 0.8.37;

import {IAntfarmAsset, IAntfarmVenue} from "./AntfarmWallet.sol";

interface IAntfarmFundingVault {
    function flashLoan(address recipient,address[] calldata tokens,uint256[] calldata amounts,bytes calldata data) external;
}

contract AntfarmOutputWallet {
    address public constant feeToken = 0x518b63Da813D46556FEa041A88b52e3CAa8C16a8;
    IAntfarmVenue public constant router = IAntfarmVenue(0x6D9f0eb21D77C6d24bE49a579508471E937D5418);
    address public constant vault = 0xBA12222222228d8Ba445958a75a0704d566BF2C8;
    bool transient private entered;
    bytes32 transient private pending;

    struct Order {
        address input;
        address output;
        uint16 tier;
        uint16 fundingTier;
        bool supporting;
        bool useLoan;
        uint256 amount;
        uint256 minCredit;
        uint256 maxAtf;
        uint256 maxFunding;
        uint256 maxLoanFee;
        uint256 deadline;
    }

    event Settled(address indexed payer,address indexed input,address indexed output,uint256 debit,uint256 credit,
                  uint256 atfCredit,uint256 fundingDebit,uint256 loanFee);

    function balance(address asset,address who) private view returns(uint256) {
        return IAntfarmAsset(asset).balanceOf(who);
    }
    function tokenCall(address asset,bytes memory data) private {
        (bool ok,bytes memory value)=asset.call(data);
        require(asset.code.length!=0 && ok && (value.length==0 || abi.decode(value,(bool))),"token");
    }
    function approve(address asset,uint256 value) private {
        if(IAntfarmAsset(asset).allowance(address(this),address(router))!=0)
            tokenCall(asset,abi.encodeCall(IAntfarmAsset.approve,(address(router),0)));
        if(value!=0) tokenCall(asset,abi.encodeCall(IAntfarmAsset.approve,(address(router),value)));
    }
    function transfer(address asset,address recipient,uint256 amount) private {
        if(amount!=0) tokenCall(asset,abi.encodeCall(IAntfarmAsset.transfer,(recipient,amount)));
    }
    function pull(address asset,uint256 amount) private {
        tokenCall(asset,abi.encodeCall(IAntfarmAsset.transferFrom,(msg.sender,address(this),amount)));
    }
    function path(address first,address second) private pure returns(address[] memory p) {
        p=new address[](2);p[0]=first;p[1]=second;
    }
    function fees(uint16 tier) private pure returns(uint16[] memory f) {
        f=new uint16[](1);f[0]=tier;
    }
    function acquire(Order memory o,uint256 required) private returns(uint256 cost) {
        if(required==0) return 0;
        uint256 initial=balance(o.output,address(this));
        uint256 initialAtf=balance(feeToken,address(this));
        uint256 embeddedFee=required*o.fundingTier/(1000-o.fundingTier);
        approve(o.output,o.maxFunding);
        router.swapTokensForExactTokens(IAntfarmVenue.Swap(required,o.maxFunding,embeddedFee,path(o.output,feeToken),
            fees(o.fundingTier),address(this),o.deadline));
        approve(o.output,0);
        cost=initial-balance(o.output,address(this));
        require(cost<=o.maxFunding && balance(feeToken,address(this))-initialAtf==required,"funding");
    }
    function primary(Order memory o,uint256 quantity) private returns(uint256 atfSpent) {
        uint256 starting=balance(feeToken,address(this));
        approve(o.input,quantity);
        approve(feeToken,o.maxAtf);
        IAntfarmVenue.Swap memory swap=IAntfarmVenue.Swap(quantity,1,o.maxAtf,path(o.input,o.output),
            fees(o.tier),address(this),o.deadline);
        if(o.supporting) router.swapExactTokensForTokensSupportingFeeOnTransferTokens(swap);
        else router.swapExactTokensForTokens(swap);
        approve(o.input,0);
        approve(feeToken,0);
        atfSpent=starting-balance(feeToken,address(this));
        require(atfSpent<=o.maxAtf,"atf");
    }

    uint256 transient private actualFunding;
    uint256 transient private actualLoanFee;

    function receiveFlashLoan(address[] calldata tokens,uint256[] calldata amounts,uint256[] calldata premiums,
                              bytes calldata data) external {
        require(entered && msg.sender==vault && pending!=bytes32(0) && keccak256(data)==pending,"callback");
        pending=bytes32(0);
        (Order memory o,uint256 quantity,uint256 openOutput)=abi.decode(data,(Order,uint256,uint256));
        require(tokens.length==1 && amounts.length==1 && premiums.length==1 && tokens[0]==o.output &&
                amounts[0]==o.maxFunding && premiums[0]<=o.maxLoanFee,"loan");
        require(balance(o.output,address(this))==openOutput+amounts[0],"loan received");
        actualFunding=acquire(o,o.maxAtf);
        primary(o,quantity);
        transfer(o.output,vault,amounts[0]+premiums[0]);
        actualLoanFee=premiums[0];
    }

    function execute(Order calldata o) external returns(uint256 debit,uint256 credit) {
        require(!entered && block.chainid==1 && block.timestamp<=o.deadline,"order");
        require(o.amount!=0 && o.minCredit!=0 && o.maxAtf!=0 && o.maxFunding!=0 && o.fundingTier>0 &&
                o.fundingTier<1000 && o.input!=o.output && o.input!=feeToken && o.output!=feeToken,"assets");
        require(address(router).codehash==0xab093829729d83c3859f76c3343b65622c0cd740c2b40a661bc2e0b5f6a45cb4 &&
                router.antfarmToken()==feeToken,"router");
        entered=true;
        uint256 beforeIn=balance(o.input,msg.sender);
        uint256 beforeOut=balance(o.output,msg.sender);
        uint256 beforeAtf=balance(feeToken,msg.sender);
        uint256 baseIn=balance(o.input,address(this));
        uint256 baseOut=balance(o.output,address(this));
        uint256 baseAtf=balance(feeToken,address(this));
        pull(o.input,o.amount);
        uint256 quantity=balance(o.input,address(this))-baseIn;
        require(quantity!=0 && quantity<=o.amount,"input");
        uint256 funding;
        uint256 premium;
        if(o.useLoan) {
            require(vault.codehash==0x9eb70db20a41bfbf4b022fd070fa7f154b7c4aec98177120dde7a958384f4e66,"vault");
            address[] memory tokens=new address[](1);tokens[0]=o.output;
            uint256[] memory amounts=new uint256[](1);amounts[0]=o.maxFunding;
            bytes memory data=abi.encode(o,quantity,baseOut);
            pending=keccak256(data);
            IAntfarmFundingVault(vault).flashLoan(address(this),tokens,amounts,data);
            require(pending==bytes32(0),"incomplete");
            funding=actualFunding;premium=actualLoanFee;
            actualFunding=0;actualLoanFee=0;
        } else {
            require(o.maxLoanFee==0,"unused loan fee");
            pull(feeToken,o.maxAtf);
            require(balance(feeToken,address(this))==baseAtf+o.maxAtf,"working capital");
            uint256 spent=primary(o,quantity);
            uint256 gross=balance(o.output,address(this))-baseOut;
            require(gross>=o.minCredit,"output");
            funding=acquire(o,spent);
        }
        transfer(feeToken,msg.sender,balance(feeToken,address(this))-baseAtf);
        transfer(o.output,msg.sender,balance(o.output,address(this))-baseOut);
        require(balance(o.input,address(this))==baseIn && balance(o.output,address(this))==baseOut &&
                balance(feeToken,address(this))==baseAtf,"remaining balance");
        debit=beforeIn-balance(o.input,msg.sender);
        credit=balance(o.output,msg.sender)-beforeOut;
        uint256 atfCredit=balance(feeToken,msg.sender)-beforeAtf;
        require(debit==o.amount && credit>=o.minCredit && (o.useLoan || atfCredit==0),"payer limits");
        entered=false;
        emit Settled(msg.sender,o.input,o.output,debit,credit,atfCredit,funding,premium);
    }
}
