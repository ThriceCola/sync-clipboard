CARGO ?= cargo
TOOLCHAIN ?= nightly
SCCACHE := $(shell command -v sccache 2>/dev/null)

ifneq ($(SCCACHE),)
USE_SCCACHE ?= 1
else
USE_SCCACHE ?= 0
endif

.PHONY: nc nb nr rr ncc c b r cc xwin sql_update

nc:
	USE_SCCACHE=$(USE_SCCACHE) $(CARGO) +$(TOOLCHAIN) check

nb:
	USE_SCCACHE=$(USE_SCCACHE) $(CARGO) +$(TOOLCHAIN) build

nr:
	USE_SCCACHE=$(USE_SCCACHE) $(CARGO) +$(TOOLCHAIN) run -p eom-server --bin eom-server

rr:
	USE_SCCACHE=$(USE_SCCACHE) $(CARGO) +$(TOOLCHAIN) run -p eom-server --bin eom-server --release

ncc:
	USE_SCCACHE=$(USE_SCCACHE) $(CARGO) +$(TOOLCHAIN) clippy

# Short aliases
c: nc
b: nb
r: nr
cc: ncc

# Windows MSVC 交叉编译（使用 stable toolchain + cargo-xwin）
# 先确保：
# 1) 用 cargo 安装 cargo-xwin（自动安装）
# 2) 安装 Rust msvc target：x86_64-pc-windows-msvc（自动添加）
# 输出发布构建于 target/x86_64-pc-windows-msvc/release/
xwin:
	command -v cargo-xwin >/dev/null 2>&1 || { echo "安装 cargo-xwin..."; rustup run stable cargo install --locked cargo-xwin; }
	rustup target list --installed | grep -q '^x86_64-pc-windows-msvc$$' || rustup target add x86_64-pc-windows-msvc
	rustup run stable cargo xwin build --release --target x86_64-pc-windows-msvc
