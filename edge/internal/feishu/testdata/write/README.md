# testdata/write

出站面（文本 / 卡片 / 文件 / 表情 / 私聊）的请求体夹具。

CC8 建了这个目录，但本轨的出站测试（`outbound_test.go` 里的每群限速、发送 uuid，`cards_test.go`
里的按钮开关）用的是内联断言，这里暂时没有夹具。归属：**DD10**（W2，`outbound.go` / `cards.go` 的主人）。
