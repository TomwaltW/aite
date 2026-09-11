// Package pin 让 go.mod 在各轨并行期间保持稳定：R1 / R2 要用的三方库在 R0 就拉进依赖表，
// 各轨只管 import，不碰 go.mod / go.sum（守卫会拦 go get / go mod）。
// 需要新依赖 → 停下报告，由 R0 / 总管加到这里再 go mod tidy。
package pin

import (
	_ "github.com/docker/docker/api/types/container"
	_ "github.com/docker/docker/client"
	_ "github.com/gorilla/websocket" // R1 的卡片帧测试直接 import 它造假 ws 服务端
	_ "github.com/larksuite/oapi-sdk-go/v3"
	_ "github.com/larksuite/oapi-sdk-go/v3/event/dispatcher"
	_ "github.com/larksuite/oapi-sdk-go/v3/service/im/v1"
	_ "github.com/larksuite/oapi-sdk-go/v3/ws"
	_ "golang.org/x/time/rate"
)
