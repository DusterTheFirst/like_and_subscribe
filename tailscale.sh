#!/bin/env bash

set -eux -o pipefail

sudo tailscale serve --set-path=/ --bg 8080
sudo tailscale funnel --set-path=/pubsub --bg 8080

sudo tailscale funnel status