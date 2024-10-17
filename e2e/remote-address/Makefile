SHELL = /usr/bin/env bash -o pipefail
.SHELLFLAGS = -ec
.DEFAULT_GOAL := gateway
MKFILE_PATH := $(abspath $(lastword $(MAKEFILE_LIST)))
WORKDIR := $(patsubst %/,%,$(dir $(MKFILE_PATH)))
DOCKER ?= $(shell which docker 2> /dev/null || echo "docker")

run:
	$(DOCKER) compose -f docker-compose.yaml run start_services

test:
	curl --silent --output /dev/null --fail --resolve test.example.com:18000:127.0.0.1 -H "X-Forwarded-For: 40.0.0.1" "http://test.example.com:18000"
	curl --silent --output /dev/null --fail --resolve test.example.com:18000:127.0.0.1 -H "X-Forwarded-For: 50.0.0.1" "http://test.example.com:18000"
	$(eval TMP := $(shell mktemp -d))
	curl --silent --output $(TMP)/counters.json --fail "http://127.0.0.1:18080/counters/ratelimit-source"
	# only one counter
	NUM_COUNTERS=$$(jq --exit-status 'length' $(TMP)/counters.json) && test $${NUM_COUNTERS} -eq 1
	# that counter must belong to 40.0.0.1
	VARIABLE_COUNTER=$$(jq -r --exit-status '.[].set_variables."source.remote_address"' $(TMP)/counters.json) && [ "$${VARIABLE_COUNTER}" == "40.0.0.1" ]

clean:
	$(DOCKER) compose down --volumes --remove-orphans
	$(DOCKER) compose -f docker-compose.yaml down --volumes --remove-orphans
