// SPDX-License-Identifier: MIT
// Исполнение покупки и продажи на кривой Virtuals с проверкой привязки пары и роли роутера.
// Библиотеку вызывает BasePmmExecutor. Экземпляр исполнителя развёртывает владелец кошелька.
pragma solidity 0.8.37;

interface IVirtualPair {
    function router() external view returns(address);
    function tokenA() external view returns(address);
    function tokenB() external view returns(address);
}
interface IVirtualFactory {
    function getPair(address token,address asset) external view returns(address);
}
interface IVirtualRouter {
    function factory() external view returns(address);
    function assetToken() external view returns(address);
    function hasRole(bytes32 role,address who) external view returns(bool);
}
interface IVirtualBonding {
    function factory() external view returns(address);
    function router() external view returns(address);
    function buy(uint256 amount,address token,uint256 minimum,uint256 deadline) external returns(bool);
    function sell(uint256 amount,address token,uint256 minimum,uint256 deadline) external returns(bool);
}

/// Public bonding contracts settle to msg.sender. The caller measures its new token balance and pays the signer.
library BaseVirtuals {
    address internal constant ASSET=address(bytes20(hex"0b3e328455c4059eeb9e3f84b5543f74e24e7e1b"));
    address private constant CLASSIC=address(bytes20(hex"8292b43ab73efac11faf357419c38acf448202c5"));
    address private constant UNICORN=address(bytes20(hex"bfce3fbe9ce3a19adb8dbb096ea2cb2bb1073f95"));
    address private constant CLASSIC_FACTORY=address(bytes20(hex"158d7ccaa23dc3c8861c3323ed546e3d25e74309"));
    address private constant UNICORN_FACTORY=address(bytes20(hex"d7d3c85b4f2e9bee1998cd2e98820e647792d284"));
    address private constant BOND=address(bytes20(hex"f66dea7b3e897cd44a5a231c61b6b4423d613259"));
    address private constant BOND2=address(bytes20(hex"c3538ddd84619e761b4c03caf2f785f79889958d"));
    address private constant BOND4=address(bytes20(hex"a31bd6a0edbc4da307b8fa92bd6cf39e0fae262c"));
    error ClosedMarket();

    function open(address entry,address pool,address token) private view returns(bool) {
        // Verified tokenInfo getter on all three entry contracts. Only the fixed return prefix is needed.
        (bool ok,bytes memory out)=entry.staticcall(abi.encodeWithSignature("tokenInfo(address)",token));
        if (!ok || out.length < (entry == BOND ? 416 : 544)) return false;
        address registered;address pair;uint256 trading;uint256 graduated;
        assembly ("memory-safe") {
            registered := mload(add(out,64))
            pair := mload(add(out,96))
            trading := mload(add(out,384))
            graduated := mload(add(out,416))
        }
        if (entry != BOND) {
            uint256 launched;
            assembly ("memory-safe") { launched := mload(add(out,544)) }
            if (launched != 1) return false;
        }
        return registered==token && pair==pool && trading==1 && graduated==0;
    }

    function market(address pool,address a,address b) internal view returns(address router,address entry) {
        address token=a==ASSET ? b : b==ASSET ? a : address(0);
        if(token==address(0) || token==ASSET) return (address(0),address(0));
        (bool ok,bytes memory raw)=pool.staticcall(abi.encodeCall(IVirtualPair.router,()));
        if(!ok || raw.length!=32) return (address(0),address(0));
        router=abi.decode(raw,(address));
        if(router!=CLASSIC && router!=UNICORN) return (address(0),address(0));
        address factory=router==CLASSIC ? CLASSIC_FACTORY : UNICORN_FACTORY;
        if(IVirtualRouter(router).factory()!=factory || IVirtualRouter(router).assetToken()!=ASSET ||
           IVirtualFactory(factory).getPair(token,ASSET)!=pool || IVirtualPair(pool).tokenA()!=token ||
           IVirtualPair(pool).tokenB()!=ASSET) revert ClosedMarket();
        entry=router==CLASSIC ? BOND : BOND4;
        if(!open(entry,pool,token)) {
            if(router!=UNICORN || !open(BOND2,pool,token)) revert ClosedMarket();
            entry=BOND2;
        }
        if(IVirtualBonding(entry).factory()!=factory || IVirtualBonding(entry).router()!=router ||
           !IVirtualRouter(router).hasRole(keccak256("EXECUTOR_ROLE"),entry)) revert ClosedMarket();
    }

    function swap(address entry,address pool,address a,address b,uint256 amount,uint256 minimum,uint256 deadline) internal {
        bool buy=a==ASSET;
        address token=buy?b:a;
        bool ok=buy?IVirtualBonding(entry).buy(amount,token,minimum,deadline):IVirtualBonding(entry).sell(amount,token,minimum,deadline);
        if(!ok || !open(entry,pool,token)) revert ClosedMarket();
    }
}
