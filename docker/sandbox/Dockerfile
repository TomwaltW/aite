# Aite P0 沙箱镜像（owner: T3；2026-09-28 CC12 加中国版构建参数、shell 工具、运行期镜像源、AIGC 标识器）
#
#   docker build -t aite-sandbox:p0 docker/sandbox
#
# 这个镜像是 §6-T3 那行验收的对象，四条硬要求：
#   1. import pandas, matplotlib, openpyxl, docx 全部成功
#   2. fc-list :lang=zh 非空（中文字体，M3 演示的成败点）
#   3. 无网络也能正常跑（网络由 DockerSandbox 用 network_mode=none 断掉，
#      镜像本身不做网络相关的事）
#   4. 有 coreutils timeout —— DockerSandbox 靠它在容器内强制执行 exec 超时；
#      少了它超时就只能靠宿主取消，容器里会留个跑满 CPU 的孤儿
#
# 不写 --platform：本机是 aarch64，指定 amd64 基底会让 pandas/matplotlib
# 走 QEMU 模拟，慢到没法演示。
#
# ---- 镜像构建参数（2026-09-28 CC12）----------------------------------------------
# 不传 = 上游、与加它们之前逐字等价。BASE_REGISTRY / APT_MIRROR 的名字、默认值、语义与
# docker/core、docker/edge 两份 Dockerfile（CC1）逐字一致——`make compose-build` 的全局
# --build-arg 也会发到这里（docker-compose.yml 的 sandbox-image）：
#   BASE_REGISTRY  基础镜像仓库（默认 docker.io），只在 FROM 行里用 → 只要这一条全局声明。
#                  各云厂商 / 客户 Harbor 的取值由 EE14 写进 deploy.md。
#   APT_MIRROR     空 = 不动 apt 源；非空 = 镜像主机名（不是 URL），把 debian.sources 里的
#                  deb.debian.org（trixie 的 debian-security 也在这个主机上）与 security.debian.org 都换掉。
#                  国内取值：mirrors.tuna.tsinghua.edu.cn / mirrors.aliyun.com
#   PIP_INDEX_URL  构建期 pip 的索引（默认 https://pypi.org/simple；空串也回落上游）。
#                  国内取值：https://pypi.tuna.tsinghua.edu.cn/simple / https://mirrors.aliyun.com/pypi/simple/
# 运行期的 pip / npm / Go / cargo 国内源是另一回事，见下面「运行期镜像源」那层。
ARG BASE_REGISTRY=docker.io
FROM ${BASE_REGISTRY}/library/python:3.11-slim
ARG APT_MIRROR=
ARG PIP_INDEX_URL=https://pypi.org/simple

# --- 系统层：字体 + fontconfig + shell 工具 ------------------------------------
# fonts-wqy-microhei 只有几 MB，CJK 覆盖够 P0 用；fonts-noto-cjk 质量更好但装完
# 要多 250MB，P0 不值这个体积。fontconfig 提供验收命令用的 fc-list。
# git curl jq unzip 是 D12 批的（CT12：镜像原先没有 git）；ca-certificates bash 在 slim 基底里本来就有，按原卡照列。
RUN if [ -n "${APT_MIRROR}" ]; then \
      sed -i -e "s#deb\.debian\.org#${APT_MIRROR}#g" -e "s#security\.debian\.org#${APT_MIRROR}#g" \
        /etc/apt/sources.list.d/debian.sources; \
    fi \
 && apt-get update \
 && apt-get install -y --no-install-recommends \
      fontconfig \
      fonts-wqy-microhei \
      fonts-wqy-zenhei \
      git \
      curl \
      jq \
      unzip \
      ca-certificates \
      bash \
 && rm -rf /var/lib/apt/lists/* \
 && fc-cache -f \
 && test -n "$(fc-list :lang=zh)" \
 && command -v timeout >/dev/null \
 && git --version && jq --version

# --- Python 层 ---------------------------------------------------------------
COPY requirements.txt /tmp/requirements.txt
RUN pip install --no-cache-dir --root-user-action=ignore \
      --index-url "${PIP_INDEX_URL:-https://pypi.org/simple}" \
      -r /tmp/requirements.txt \
 && rm /tmp/requirements.txt

# 中文字体的默认接线，理由见 matplotlibrc 里的注释。
COPY matplotlibrc /etc/aite/matplotlibrc
ENV MATPLOTLIBRC=/etc/aite/matplotlibrc

# --- 运行期镜像源（network=none 下不起作用；等 china-trusted 档放行这些主机后生效）----
# ⚠️ 顺序陷阱：/etc/pip.conf 必须在上面 pip install 那层**之后**才 COPY，否则构建期的 pip
# 先读到清华，PIP_INDEX_URL 形同虚设。后来的 EE10 / FF8 要加 pip install：要么放在这层之前，
# 要么显式带 --index-url "${PIP_INDEX_URL:-https://pypi.org/simple}"。
# 不设任何 HTTP(S)_PROXY / ALL_PROXY：那不是「惰性」，还会改掉 TestImageHasNoNetworkEgress 看到的报错形态。
COPY mirrors/pip.conf /etc/pip.conf
ENV GOPROXY=https://goproxy.cn,direct

# --- AIGC 隐式标识器（DD5 在 fetch_artifact 之前调；接口见 aite/aite_label.py 的 docstring）---
# 以 root 放进 /opt/aite：文件 0644、目录 0755，aite 用户只读。
COPY aite/ /opt/aite/
RUN find /opt/aite -type d -exec chmod 0755 {} + \
 && find /opt/aite -type f -exec chmod 0644 {} +

# --- 运行身份 ---------------------------------------------------------------
# 不用 root 跑模型写的代码。uid 钉成 1000：DockerSandbox.put_file 往容器里塞文件时
# tar 头里写的就是 1000，两边对不上的话沙箱内的代码改不动自己的输入文件。
RUN useradd --create-home --uid 1000 aite \
 && mkdir -p /work /work/in /home/aite/.cargo \
 && chown -R aite:aite /work /home/aite/.cargo

# /home/aite 下的运行期镜像源：必须在 useradd --create-home 之后，属主 aite。
COPY --chown=1000:1000 mirrors/npmrc /home/aite/.npmrc
COPY --chown=1000:1000 mirrors/cargo-config.toml /home/aite/.cargo/config.toml

ENV HOME=/home/aite \
    MPLCONFIGDIR=/home/aite/.cache/matplotlib \
    PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1

USER aite
WORKDIR /work

# 出厂自检 + 预热 matplotlib 的字体缓存（第一次 import pyplot 要扫全部字体，
# 放到 build 期做掉，演示时第一张图就不会多等两秒）。
# 第一行的四个 import 逐字保留（preflight 第 6 组与 docker_test.go 钉的就是这四个）；
# PIL / pypdf / 标识器另起一行。
RUN python -c "import pandas, matplotlib, openpyxl, docx" \
 && python -c "import PIL, pypdf, sys; sys.path.insert(0, '/opt/aite'); import aite_label" \
 && python -c "import matplotlib.pyplot as plt; f=plt.figure(); plt.title('中文预热'); f.canvas.draw()"

# DockerSandbox 会显式传 command，这里只是给 `docker run` 一个不会立刻退出的默认值。
CMD ["sleep", "infinity"]
