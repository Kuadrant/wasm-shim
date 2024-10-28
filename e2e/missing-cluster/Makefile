SHELL = /usr/bin/env bash -o pipefail
.SHELLFLAGS = -ec
.DEFAULT_GOAL := gateway
MKFILE_PATH := $(abspath $(lastword $(MAKEFILE_LIST)))
WORKDIR := $(patsubst %/,%,$(dir $(MKFILE_PATH)))
DOCKER ?= $(shell which docker 2> /dev/null || echo "docker")

run:
	$(DOCKER) compose -f docker-compose.yaml run start_services

test:
	@{ \
	set -e ;\
	STATUSCODE=$(shell curl --silent --output /dev/null --write-out "%{http_code}" --max-time 5 --resolve test.example.com:18000:127.0.0.1 http://test.example.com:18000) && \
		test $${STATUSCODE} -ne 200 ;\
	}

clean:
	$(DOCKER) compose down --volumes --remove-orphans
	$(DOCKER) compose -f docker-compose.yaml down --volumes --remove-orphans
