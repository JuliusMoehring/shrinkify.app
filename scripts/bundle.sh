#!/bin/bash

rm -rf bundle

mkdir bundle

# Copy the host.json files
cp host.json bundle/

# Copy functions
cp -r create-shrink bundle/
cp -r generate-shrink-origin bundle/
cp -r redirect bundle/
cp -r validate-shrink-origin bundle/

# Copy handler
cp handler bundle/