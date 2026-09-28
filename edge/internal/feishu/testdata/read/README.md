# testdata/read

读取面（群 / 话题历史、通讯录、云文档）的响应夹具。

CC8 建了这个目录，但本轨的读取测试（`reads_test.go` 里的话题历史、发言人姓名）用的是内联 JSON，
这里暂时没有夹具。归属：**DD9**（W2，`reads.go` 的主人）往这里加夹具时，给 `helpers_test.go`
加一个读取根即可。
