// SPDX-License-Identifier: MIT
// Файл: EulerSwapWallet.sol. Кошелёк плательщика и шаг 17 ArbExecutor V5.
// У официальных фабрик Ethereum и Base собственный код без DELEGATECALL, CALLCODE и SELFDESTRUCT.
// V1 хранит заменяемую регистрацию poolByEulerAccount, V2 отмечает созданные пулы в deployedPools.
// Адреса и хеши eth_getCode: tests/data/first_run_dex_2026-11-26/eulerswap_roles.json.
// Исходник и байткод каждой фабрики: https://sourcify.dev/server/v2/contract/{chainId}/{address}?fields=all
// При каждом обмене settle заново проверяет регистрацию и активы. Расход плательщика равен paid ≤ maximum,
// фактический прирост баланса получателя ≥ minimum. Для exact-in цена считается по зачисленному пулу входу,
// для exact-out пул должен получить весь paid. Оператор сохраняет контроль над своим счётом Euler.
pragma solidity 0.8.37;

import {IBalancerVault} from "./IPools.sol";

interface IEulerAsset {
    function balanceOf(address holder) external view returns (uint256);
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
}

interface IEulerPool {
    function getAssets() external view returns (address, address);
    function computeQuote(address input, address output, uint256 amount, bool exactIn) external view returns (uint256);
    function swap(uint256 amount0Out, uint256 amount1Out, address recipient, bytes calldata data) external;
}

interface IEulerLegacyPool {
    struct Params {
        address vault0;
        address vault1;
        address eulerAccount;
        uint112 equilibriumReserve0;
        uint112 equilibriumReserve1;
        uint256 priceX;
        uint256 priceY;
        uint256 concentrationX;
        uint256 concentrationY;
        uint256 fee;
        uint256 protocolFee;
        address protocolFeeRecipient;
    }
    function getParams() external view returns (Params memory);
}

interface IEulerLegacyFactory {
    function poolByEulerAccount(address account) external view returns (address);
}
interface IEulerCurrentFactory {
    function deployedPools(address pool) external view returns (bool);
}

contract EulerSwapWallet {
    IEulerLegacyFactory public immutable legacy;
    IEulerCurrentFactory public immutable current;
    bool transient private busy;
    error Market();
    error Bounds();
    error Payment();
    event Filled(address indexed payer, address indexed recipient, address indexed pool,
                 address input, address output, uint256 paid, uint256 received);

    constructor(address v1, address v2) {
        if (v1.code.length==0 || v2.code.length==0 || v1==v2) revert Market();
        legacy=IEulerLegacyFactory(v1);
        current=IEulerCurrentFactory(v2);
    }

    function execute(address pool,address input,address output,uint256 amount,uint256 minimum,uint256 deadline)
        external returns(uint256 received) {
        (,received)=settle(pool,input,output,amount,minimum,deadline,msg.sender,false);
    }

    function executeTarget(address pool,address input,address output,uint256 maximum,uint256 wanted,uint256 deadline)
        external returns(uint256 paid,uint256 received) {
        return settle(pool,input,output,maximum,wanted,deadline,msg.sender,true);
    }

    function swap(IBalancerVault.SingleSwap calldata o,IBalancerVault.FundManagement calldata f,
                  uint256 minimum,uint256 deadline) external returns(uint256 received) {
        if (o.kind!=0 || o.userData.length!=0 || f.sender!=msg.sender || f.fromInternalBalance ||
            f.toInternalBalance || uint256(o.poolId)>type(uint160).max) revert Bounds();
        (,received)=settle(address(uint160(uint256(o.poolId))),o.assetIn,o.assetOut,o.amount,
                          minimum,deadline,f.recipient,false);
    }

    function settle(address pool,address input,address output,uint256 maximum,uint256 minimum,uint256 deadline,
                    address recipient,bool exactOut) private returns(uint256 paid,uint256 received) {
        if (busy || input==output || maximum==0 || minimum==0 || deadline<block.timestamp ||
            recipient==address(0) || recipient==pool || recipient==address(this) || msg.sender==pool)
            revert Bounds();
        busy=true;
        if (!current.deployedPools(pool) &&
            legacy.poolByEulerAccount(IEulerLegacyPool(pool).getParams().eulerAccount)!=pool) revert Market();
        IEulerPool market=IEulerPool(pool);
        (address a,address b)=market.getAssets();
        if (!((input==a && output==b) || (input==b && output==a))) revert Market();
        IEulerAsset src=IEulerAsset(input);
        IEulerAsset dst=IEulerAsset(output);
        uint256 ownerIn=src.balanceOf(msg.sender);
        uint256 poolIn=src.balanceOf(pool);
        uint256 ownerOut=dst.balanceOf(recipient);
        paid=exactOut ? market.computeQuote(input,output,minimum,false) : maximum;
        if (paid==0 || paid>maximum) revert Bounds();
        (bool ok,bytes memory ret)=input.call(abi.encodeCall(src.transferFrom,(msg.sender,pool,paid)));
        if (!ok || (ret.length!=0 && !abi.decode(ret,(bool)))) revert Payment();
        uint256 credited=src.balanceOf(pool)-poolIn;
        if (credited==0 || credited>paid || (exactOut && credited!=paid)) revert Payment();
        uint256 wanted=exactOut ? minimum : market.computeQuote(input,output,credited,true);
        if (wanted<minimum) revert Bounds();
        market.swap(output==a ? wanted : 0,output==b ? wanted : 0,recipient,"");
        received=dst.balanceOf(recipient)-ownerOut;
        if (ownerIn-src.balanceOf(msg.sender)!=paid || received<minimum) revert Payment();
        emit Filled(msg.sender,recipient,pool,input,output,paid,received);
        busy=false;
    }
}

