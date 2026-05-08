DEVICE_PATH := /tmp/smon-fake

.PHONY: device

device:
	@command -v socat >/dev/null || { echo "socat not found. install with: brew install socat"; exit 1; }
	@echo "Fake serial device at: $(DEVICE_PATH)"
	@echo "In another terminal: cargo run, then type the path above at the port picker."
	@echo "Type lines here to send to smon. Ctrl+C to stop."
	@echo
	@socat -d pty,raw,echo=0,link=$(DEVICE_PATH) -
