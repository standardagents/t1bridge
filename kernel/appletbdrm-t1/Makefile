KDIR ?= /lib/modules/$(shell uname -r)/build

.PHONY: all clean test sanitize

obj-m += appletbdrm.o

all:
	$(MAKE) -C $(KDIR) M=$(CURDIR) modules

clean:
	$(MAKE) -C $(KDIR) M=$(CURDIR) clean

test:
	@set -eu; binary=$$(mktemp); trap 'rm -f "$$binary"' EXIT; \
	for source in test_appletbdrm_resume.c test_appletbdrm_protocol.c; do \
	$(CC) -O2 -std=c17 -Wall -Wextra -Wpedantic -Werror \
		"$$source" -o "$$binary"; "$$binary"; done

sanitize:
	@set -eu; binary=$$(mktemp); trap 'rm -f "$$binary"' EXIT; \
	for source in test_appletbdrm_resume.c test_appletbdrm_protocol.c; do \
	gcc -O1 -g -std=c17 -Wall -Wextra -Wpedantic -Werror \
		-fno-omit-frame-pointer -fno-sanitize-recover=all \
		-fsanitize=address,undefined "$$source" \
		-o "$$binary"; "$$binary"; done
