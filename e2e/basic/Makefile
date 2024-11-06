SHELL = /usr/bin/env bash -o pipefail
.SHELLFLAGS = -ec
.DEFAULT_GOAL := gateway
MKFILE_PATH := $(abspath $(lastword $(MAKEFILE_LIST)))
WORKDIR := $(patsubst %/,%,$(dir $(MKFILE_PATH)))
DOCKER ?= $(shell which docker 2> /dev/null || echo "docker")

run:
	$(DOCKER) compose -f docker-compose.yaml run start_services

test:
	curl --silent --output /dev/null --fail --resolve test.example.com:18000:127.0.0.1 "http://test.example.com:18000"
	$(eval TMP := $(shell mktemp -d))
	curl --silent --output $(TMP)/counters.json --fail "http://127.0.0.1:18080/counters/basic"
	# only one counter
	NUM_COUNTERS=$$(jq --exit-status 'length' $(TMP)/counters.json) && test $${NUM_COUNTERS} -eq 1
	# check counter value
	COUNTER=$$(jq -r --exit-status '.[0].remaining' $(TMP)/counters.json) && [ "$${COUNTER}" == "28" ]

clean:
	$(DOCKER) compose down --volumes --remove-orphans
	$(DOCKER) compose -f docker-compose.yaml down --volumes --remove-orphans
