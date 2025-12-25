# syntax=docker/dockerfile:1
#checkov:skip=CKV_DOCKER_2
#checkov:skip=CKV_DOCKER_3
FROM centos:7 AS builder

ARG PHP_VERSION='8.5'
ENV PHP_VERSION=${PHP_VERSION}

SHELL ["/bin/bash", "-o", "pipefail", "-c"]
# yum update
RUN sed -i 's/mirror.centos.org/vault.centos.org/g' /etc/yum.repos.d/*.repo && \
	sed -i 's/^#.*baseurl=http/baseurl=http/g' /etc/yum.repos.d/*.repo && \
	sed -i 's/^mirrorlist=http/#mirrorlist=http/g' /etc/yum.repos.d/*.repo && \
	yum clean all && \
	yum makecache && \
	yum update -y && \
	yum install -y centos-release-scl

# different arch for different scl repo
RUN if [ "$(uname -m)" = "aarch64" ]; then \
		sed -i 's|mirror.centos.org/centos|vault.centos.org/altarch|g' /etc/yum.repos.d/CentOS-SCLo-scl-rh.repo ; \
		sed -i 's|mirror.centos.org/centos|vault.centos.org/altarch|g' /etc/yum.repos.d/CentOS-SCLo-scl.repo ; \
		sed -i 's/^#.*baseurl=http/baseurl=http/g' /etc/yum.repos.d/*.repo ; \
		sed -i 's/^mirrorlist=http/#mirrorlist=http/g' /etc/yum.repos.d/*.repo ; \
	else \
		sed -i 's/mirror.centos.org/vault.centos.org/g' /etc/yum.repos.d/*.repo ; \
		sed -i 's/^#.*baseurl=http/baseurl=http/g' /etc/yum.repos.d/*.repo ; \
		sed -i 's/^mirrorlist=http/#mirrorlist=http/g' /etc/yum.repos.d/*.repo ; \
	fi; \
	yum update -y && \
	yum install -y devtoolset-10-gcc-* && \
	echo "source scl_source enable devtoolset-10" >> /etc/bashrc && \
	source /etc/bashrc

# install build essentials
RUN yum install -y \
		perl \
		make \
		bison \
		flex \
		git \
		autoconf \
		automake \
		tar \
		unzip \
		gzip \
		gcc \
		bzip2 \
		patch \
		xz \
		libtool \
		perl-IPC-Cmd ; \
	curl -o make.tar.gz -fsSL https://ftp.gnu.org/gnu/make/make-4.4.tar.gz && \
	tar -zxvf make.tar.gz && \
	cd make-* && \
	./configure && \
	make && \
	make install && \
	ln -sf /usr/local/bin/make /usr/bin/make && \
	cd .. && \
	rm -Rf make* && \
	curl -o cmake.tar.gz -fsSL https://github.com/Kitware/CMake/releases/download/v4.1.2/cmake-4.1.2-linux-$(uname -m).tar.gz && \
	mkdir /cmake && \
	tar -xzf cmake.tar.gz -C /cmake --strip-components 1 && \
	rm cmake.tar.gz && \
	curl -fsSL -o patchelf.tar.gz https://github.com/NixOS/patchelf/releases/download/0.18.0/patchelf-0.18.0-$(uname -m).tar.gz && \
	mkdir -p /patchelf && \
	tar -xzf patchelf.tar.gz -C /patchelf --strip-components=1 && \
	cp /patchelf/bin/patchelf /usr/bin/ && \
	rm patchelf.tar.gz && \
	if [ "$(uname -m)" = "aarch64" ]; then \
		GO_ARCH="arm64" ; \
	else \
		GO_ARCH="amd64" ; \
	fi; \
	curl -o /usr/local/bin/jq -fsSL https://github.com/jqlang/jq/releases/download/jq-1.7.1/jq-linux-${GO_ARCH} && \
	chmod +x /usr/local/bin/jq

ENV PATH="/opt/rh/devtoolset-10/root/usr/bin:/cmake/bin:$PATH"

# Apply GNU mode
ENV SPC_DEFAULT_C_FLAGS='-fPIE -fPIC -O3'
ENV SPC_LIBC='glibc'
ENV SPC_CMD_VAR_PHP_MAKE_EXTRA_LDFLAGS_PROGRAM='-Wl,-O3 -pie'
ENV SPC_CMD_VAR_PHP_MAKE_EXTRA_LIBS='-ldl -lpthread -lm -lresolv -lutil -lrt'
ENV SPC_REL_TYPE='binary'

RUN curl -o /usr/local/bin/spc -fsSL "https://dl.static-php.dev/static-php-cli/spc-bin/nightly/spc-linux-$(uname -m)" && \
	chmod +x /usr/local/bin/spc

ARG PHP_EXTENSIONS="bz2,redis,bcmath,calendar,ctype,curl,dom,exif,fileinfo,filter,gd,iconv,intl,mbregex,mbstring,mysqli,mysqlnd,opcache,openssl,pcntl,pdo,pdo_mysql,pdo_sqlite,phar,posix,session,simplexml,sockets,sodium,sqlite3,tokenizer,xml,xmlreader,xmlwriter,zip,zlib,zstd"
ARG PHP_BUILD_LIBS="libavif,libwebp,libjpeg,freetype,nghttp2,brotli"

RUN --mount=type=secret,id=GITHUB_TOKEN,env=GITHUB_TOKEN spc download \
    --with-php=${PHP_VERSION} \
    --for-extensions="${PHP_EXTENSIONS}" \
    --for-libs="${PHP_BUILD_LIBS}" \
    --prefer-pre-built

RUN spc doctor --auto-fix

RUN spc build ${PHP_EXTENSIONS} \
    --build-embed \
    --enable-zts \
    --with-libs="${PHP_BUILD_LIBS}"

# Install Rust
RUN curl https://sh.rustup.rs -sSf | sh -s -- -y --profile minimal
ENV PATH="/root/.cargo/bin:${PATH}"

# Set up environment for Rust builds
ENV PHP_CONFIG=/buildroot/bin/php-config
ENV PKG_CONFIG_PATH=/buildroot/lib/pkgconfig
ENV OPENSSL_DIR=/buildroot
ENV OPENSSL_STATIC=1
ENV OPENSSL_LIB_DIR=/buildroot/lib
ENV OPENSSL_INCLUDE_DIR=/buildroot/include

WORKDIR /work
