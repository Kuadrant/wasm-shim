#!/usr/bin/env bash

if ! command -v curl &>/dev/null 
then
    echo "curl not found..." >&2
    exit 1
fi

curl --silent --output /dev/null --fail --max-time 5 --resolve test.example.com:18000:127.0.0.1 http://test.example.com:18000 & REQ1=$!
curl --silent --output /dev/null --fail --max-time 5 --resolve test.example.com:18000:127.0.0.1 http://test.example.com:18000 & REQ2=$!
curl --silent --output /dev/null --fail --max-time 5 --resolve test.example.com:18000:127.0.0.1 http://test.example.com:18000 & REQ3=$!

wait $REQ1
RET_REQ1=$?
echo "REQ1 returned $RET_REQ1"
if test "$RET_REQ1" != "0"; then
  echo "REQ1 exited with abnormal status"
  exit 1;
fi

wait $REQ2
RET_REQ2=$?
echo "REQ2 returned $RET_REQ2"
if test "$RET_REQ2" != "0"; then
  echo "REQ2 exited with abnormal status"
  exit 1;
fi

wait $REQ3
RET_REQ3=$?
echo "REQ3 returned $RET_REQ3"
if test "$RET_REQ3" != "0"; then
  echo "REQ3 exited with abnormal status"
  exit 1;
fi

echo "All requests succeeded"
exit 0
