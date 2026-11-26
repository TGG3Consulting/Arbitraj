// SPDX-License-Identifier: MIT
pragma solidity 0.8.37;

// Шаг swapFrom V5. Котируемый вход и выход — ERC-20; ETH для комиссии продажи
// предоставляет владелец модуля, и каждая успешная продажа восстанавливает его остаток.
interface IVistaRouter {
    function factory() external view returns (address);
    function WETH() external view returns (address);
    function usdcToEth(uint256 amount) external view returns (uint256);
    function swapExactETHForTokensSupportingFeeOnTransferTokens(
        uint256 minimum, address[] calldata path, address recipient, uint256 deadline
    ) external payable;
    function swapExactTokensForETHSupportingFeeOnTransferTokens(
        uint256 amount, uint256 minimum, address[] calldata path, address recipient, uint256 deadline
    ) external payable;
}

interface IVistaFactory {
    function router() external view returns (address);
    function getPair(address tokenIn, address tokenOut) external view returns (address);
}

interface IVistaPair {
    function factory() external view returns (address);
    function buyTotalFee() external view returns (uint256);
    function sellTotalFee() external view returns (uint256);
}

interface IVistaToken {
    function balanceOf(address who) external view returns (uint256);
    function allowance(address payer, address spender) external view returns (uint256);
    function approve(address spender, uint256 amount) external returns (bool);
    function transferFrom(address payer, address to, uint256 amount) external returns (bool);
}

interface IVistaWeth is IVistaToken {
    function deposit() external payable;
    function withdraw(uint256 amount) external;
    function transfer(address to, uint256 amount) external returns (bool);
}

contract EtherVistaAdapter {
    address public immutable owner;
    IVistaRouter public immutable router;
    IVistaFactory public immutable factory;
    IVistaWeth public immutable weth;
    uint256 public feeReserve;
    bool transient private entered;
    bool transient private selling;
    uint256 transient private routerPayment;
    event NativeSettled(address indexed payer, address indexed recipient, address indexed tokenIn,
                        address tokenOut, uint256 spent, uint256 received, uint256 fee);

    constructor(IVistaRouter market, address reserveOwner) {
        require(address(market).code.length != 0 && reserveOwner != address(0), "configuration");
        router = market;
        factory = IVistaFactory(market.factory());
        weth = IVistaWeth(market.WETH());
        require(address(weth).code.length != 0 && factory.router() == address(market), "router");
        owner = reserveOwner;
    }

    receive() external payable {
        if (entered && !selling && msg.sender == address(weth)) return;
        require(entered && selling && msg.sender == address(router), "source");
        routerPayment += msg.value;
    }

    function fundReserve() external payable {
        require(msg.sender == owner && !entered && msg.value != 0, "owner");
        feeReserve += msg.value;
    }

    function withdrawReserve(uint256 amount) external {
        require(msg.sender == owner && !entered && amount <= feeReserve, "reserve");
        entered = true;
        feeReserve -= amount;
        (bool ok,) = payable(owner).call{value:amount}("");
        require(ok, "repayment");
        entered = false;
    }

    function swapFrom(address tokenIn, address tokenOut, uint256 amount, uint256 minimum,
                      address recipient, uint256 deadline) external returns (uint256 received) {
        (address[] memory path, IVistaPair pair, bool buy) = start(tokenIn,tokenOut,amount,recipient,deadline);
        IVistaToken input = IVistaToken(tokenIn);
        IVistaToken output = IVistaToken(tokenOut);
        uint256 beforeOut = output.balanceOf(recipient);
        (uint256 balance, uint256 actual) = pull(input, amount);
        if (buy) {
            require(actual == amount && amount > router.usdcToEth(pair.buyTotalFee()), "buy fee");
            weth.withdraw(amount);
            router.swapExactETHForTokensSupportingFeeOnTransferTokens{value:amount}(minimum,path,recipient,deadline);
        } else {
            uint256 fee = router.usdcToEth(pair.sellTotalFee());
            require(fee <= feeReserve, "fee reserve");
            uint256 net = sell(input,pair,path,actual,minimum,fee,deadline);
            weth.deposit{value:net}();
            require(weth.transfer(recipient,net), "delivery");
        }
        require(input.balanceOf(address(this)) == balance && address(this).balance >= feeReserve, "residue");
        received = output.balanceOf(recipient) - beforeOut;
        require(received > 0 && received >= minimum, "payout");
        entered = false;
    }

    function swapNative(address tokenIn, address tokenOut, uint256 amount, uint256 minimum,
                        address recipient, uint256 deadline) external payable returns (uint256 received) {
        (address[] memory path, IVistaPair pair, bool buy) = start(tokenIn,tokenOut,amount,recipient,deadline);
        uint256 fee;
        if (buy) {
            fee = router.usdcToEth(pair.buyTotalFee());
            require(msg.value == amount && amount > fee, "buy budget");
            IVistaToken output = IVistaToken(tokenOut);
            uint256 beforeOut = output.balanceOf(recipient);
            router.swapExactETHForTokensSupportingFeeOnTransferTokens{value:amount}(minimum,path,recipient,deadline);
            received = output.balanceOf(recipient) - beforeOut;
        } else {
            IVistaToken input = IVistaToken(tokenIn);
            (uint256 beforeIn,uint256 actual) = pull(input,amount);
            fee = router.usdcToEth(pair.sellTotalFee());
            require(msg.value == fee, "sell advance");
            received = sell(input,pair,path,actual,minimum,fee,deadline);
            require(input.balanceOf(address(this)) == beforeIn, "residue");
            uint256 gross = received + fee;
            uint256 beforeOut = recipient.balance;
            (bool ok,) = payable(recipient).call{value:gross}("");
            require(ok && recipient.balance - beforeOut == gross, "native delivery");
        }
        require(received != 0 && received >= minimum && address(this).balance >= feeReserve, "payout");
        emit NativeSettled(msg.sender,recipient,tokenIn,tokenOut,amount,received,fee);
        entered = false;
    }

    function start(address tokenIn, address tokenOut, uint256 amount, address recipient, uint256 deadline)
        private returns (address[] memory path, IVistaPair pair, bool buy) {
        require(!entered && block.timestamp <= deadline && amount != 0 && tokenIn != tokenOut, "order");
        entered = true;
        require(recipient != address(0) && recipient != address(this) && factory.router() == address(router), "route");
        buy = tokenIn == address(weth);
        require(buy || tokenOut == address(weth), "weth");
        pair = IVistaPair(factory.getPair(tokenIn,tokenOut));
        require(address(pair) != address(0) && pair.factory() == address(factory), "market");
        path = new address[](2);
        path[0] = tokenIn;
        path[1] = tokenOut;
    }

    function pull(IVistaToken input,uint256 amount) private returns(uint256 balance,uint256 actual) {
        balance = input.balanceOf(address(this));
        uint256 payerBefore = input.balanceOf(msg.sender);
        checked(address(input),abi.encodeCall(input.transferFrom,(msg.sender,address(this),amount)));
        actual = input.balanceOf(address(this)) - balance;
        require(actual > 0 && actual <= amount && payerBefore-input.balanceOf(msg.sender) == amount, "spent");
    }

    function sell(IVistaToken input, IVistaPair pair, address[] memory path, uint256 amount,
                  uint256 minimum, uint256 fee, uint256 deadline) private returns (uint256 net) {
        require(input.allowance(address(this),address(router)) == 0, "allowance");
        uint256 dust = weth.balanceOf(address(router));
        uint256 pairBefore = weth.balanceOf(address(pair));
        checked(address(input), abi.encodeCall(input.approve,(address(router),amount)));
        selling = true;
        routerPayment = 0;
        router.swapExactTokensForETHSupportingFeeOnTransferTokens{value:fee}(
            amount, dust + fee + minimum, path, address(this), deadline
        );
        uint256 gross = routerPayment;
        routerPayment = 0;
        selling = false;
        if (input.allowance(address(this),address(router)) != 0)
            checked(address(input), abi.encodeCall(input.approve,(address(router),0)));
        require(input.allowance(address(this),address(router)) == 0 && gross > dust + fee, "settlement");
        require(weth.balanceOf(address(router)) == 0 && gross - dust == pairBefore - weth.balanceOf(address(pair)), "pool payout");
        net = gross - dust - fee;
        require(net >= minimum, "floor");
        if (dust != 0) {
            weth.deposit{value:dust}();
            require(weth.transfer(address(router),dust), "router balance");
        }
        require(weth.balanceOf(address(router)) == dust, "router balance");
    }

    function checked(address token, bytes memory data) private {
        (bool ok, bytes memory result) = token.call(data);
        require(ok && (result.length == 0 || abi.decode(result,(bool))), "erc20");
    }
}
