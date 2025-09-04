#!/bin/bash

set -eux -o pipefail

cd "$(dirname "$0")/src/"
rm -- *.rs
sea-orm-cli generate entity -l
