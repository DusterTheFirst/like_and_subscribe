#!/bin/env bash

set -eux -o pipefail

export CONFIG='
receivers:
  otlp:
    protocols:
      grpc:
        endpoint: 0.0.0.0:4317
      http:

exporters:
  prometheus:
    endpoint: "0.0.0.0:9999"

service:
  pipelines:
    metrics:
      receivers: [otlp]
      exporters: [prometheus]
'

docker run -p 4317:4317 -p 9999:9999 -e CONFIG -i --rm otel/opentelemetry-collector:latest --config=env:CONFIG