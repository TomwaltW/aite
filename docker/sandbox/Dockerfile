# Aite P0 沙箱镜像（owner: T3）
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
FROM python:3.11-slim

# --- 系统层：字体 + fontconfig ------------------------------------------------
# fonts-wqy-microhei 只有几 MB，CJK 覆盖够 P0 用；fonts-noto-cjk 质量更好但装完
# 要多 250MB，P0 不值这个体积。fontconfig 提供验收命令用的 fc-list。
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      fontconfig \
      fonts-wqy-microhei \
      fonts-wqy-zenhei \
 && rm -rf /var/lib/apt/lists/* \
 && fc-cache -f \
 && test -n "$(fc-list :lang=zh)" \
 && command -v timeout >/dev/null

# --- Python 层 ---------------------------------------------------------------
COPY requirements.txt /tmp/requirements.txt
RUN pip install --no-cache-dir --root-user-action=ignore -r /tmp/requirements.txt \
 && rm /tmp/requirements.txt

# 中文字体的默认接线，理由见 matplotlibrc 里的注释。
COPY matplotlibrc /etc/aite/matplotlibrc
ENV MATPLOTLIBRC=/etc/aite/matplotlibrc

# --- 运行身份 ---------------------------------------------------------------
# 不用 root 跑模型写的代码。uid 钉成 1000：DockerSandbox.put_file 往容器里塞文件时
# tar 头里写的就是 1000，两边对不上的话沙箱内的代码改不动自己的输入文件。
RUN useradd --create-home --uid 1000 aite \
 && mkdir -p /work /work/in \
 && chown -R aite:aite /work

ENV HOME=/home/aite \
    MPLCONFIGDIR=/home/aite/.cache/matplotlib \
    PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1

USER aite
WORKDIR /work

# 出厂自检 + 预热 matplotlib 的字体缓存（第一次 import pyplot 要扫全部字体，
# 放到 build 期做掉，演示时第一张图就不会多等两秒）。
RUN python -c "import pandas, matplotlib, openpyxl, docx" \
 && python -c "import matplotlib.pyplot as plt; f=plt.figure(); plt.title('中文预热'); f.canvas.draw()"

# DockerSandbox 会显式传 command，这里只是给 `docker run` 一个不会立刻退出的默认值。
CMD ["sleep", "infinity"]
