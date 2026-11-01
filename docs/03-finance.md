# Finance 命令 - 金融数据查询

查询股票、加密货币等金融数据。

## 数据源

- Twelve Data（股票）
- CoinGecko（加密货币）

## 子命令

### stock - 查询股票价格

```bash
viacli finance stock <股票代码>
```

示例：
```bash
# 查询苹果股票
viacli finance stock AAPL

# 查询特斯拉股票
viacli finance stock TSLA

# 查询上证指数
viacli finance stock 000001
```

### crypto - 查询加密货币价格

```bash
viacli finance crypto <币种>
```

示例：
```bash
# 查询比特币
viacli finance crypto bitcoin

# 查询以太坊
viacli finance crypto ethereum

# 使用简称
viacli finance crypto btc
viacli finance crypto eth
```

## 输出信息

- 当前价格
- 24小时涨跌幅
- 市值
- 交易量

## 注意事项

- 股票数据可能有延迟
- 加密货币数据实时更新
