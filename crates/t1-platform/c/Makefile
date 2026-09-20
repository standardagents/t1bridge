.PHONY: all test sanitize sanitize-clean clean

CC ?= cc
CPPFLAGS ?=
CFLAGS ?= -O2 -std=c17 -Wall -Wextra -Wpedantic -Werror

BUILD_DIR ?= .
TEST_BINARIES := $(BUILD_DIR)/test_t1_secret_wipe \
	$(BUILD_DIR)/test_t1_frame_memfd \
	$(BUILD_DIR)/test_t1_usb_cycle_guard \
	$(BUILD_DIR)/test_t1_touchbar_drm \
	$(BUILD_DIR)/test_t1_touchbar_session \
	$(BUILD_DIR)/test_t1_recovery
SANITIZER_DIR := .sanitizer
SANITIZER_BINARIES := $(SANITIZER_DIR)/test_t1_secret_wipe \
	$(SANITIZER_DIR)/test_t1_frame_memfd \
	$(SANITIZER_DIR)/test_t1_usb_cycle_guard \
	$(SANITIZER_DIR)/test_t1_touchbar_drm \
	$(SANITIZER_DIR)/test_t1_touchbar_session \
	$(SANITIZER_DIR)/test_t1_recovery
SANITIZER_CFLAGS := -O1 -g -std=c17 -Wall -Wextra -Wpedantic -Werror \
	-fno-omit-frame-pointer -fno-sanitize-recover=all \
	-fsanitize=address,undefined

all: $(TEST_BINARIES)

$(BUILD_DIR)/test_t1_recovery: t1_recovery_io.c t1_recovery_io.h t1_recovery_fs.c t1_recovery_fs.h test_t1_recovery.c | $(BUILD_DIR)
	$(CC) $(CPPFLAGS) $(CFLAGS) t1_recovery_io.c t1_recovery_fs.c test_t1_recovery.c -lcurl -larchive -lcrypto -ludev -o $@

$(BUILD_DIR)/test_t1_secret_wipe: t1_secret_wipe.c t1_secret_wipe.h test_t1_secret_wipe.c | $(BUILD_DIR)
	$(CC) $(CPPFLAGS) $(CFLAGS) t1_secret_wipe.c test_t1_secret_wipe.c -o $@

$(BUILD_DIR)/test_t1_frame_memfd: t1_frame_memfd.c t1_frame_memfd.h \
		test_t1_frame_memfd.c | $(BUILD_DIR)
	$(CC) $(CPPFLAGS) $(CFLAGS) t1_frame_memfd.c test_t1_frame_memfd.c -o $@

$(BUILD_DIR)/test_t1_usb_cycle_guard: t1_usb_cycle_guard.c \
		t1_usb_cycle_guard.h test_t1_usb_cycle_guard.c | $(BUILD_DIR)
	$(CC) $(CPPFLAGS) $(CFLAGS) t1_usb_cycle_guard.c \
		test_t1_usb_cycle_guard.c -o $@

$(BUILD_DIR)/test_t1_touchbar_drm: t1_touchbar_drm.c t1_touchbar_drm.h \
		test_t1_touchbar_drm.c | $(BUILD_DIR)
	$(CC) $(CPPFLAGS) $(CFLAGS) t1_touchbar_drm.c test_t1_touchbar_drm.c -o $@

$(BUILD_DIR)/test_t1_touchbar_session: t1_touchbar_session.c \
		t1_touchbar_session.h test_t1_touchbar_session.c | $(BUILD_DIR)
	$(CC) $(CPPFLAGS) $(CFLAGS) t1_touchbar_session.c \
		test_t1_touchbar_session.c -lsystemd -o $@

test: all
	./$(BUILD_DIR)/test_t1_secret_wipe
	./$(BUILD_DIR)/test_t1_frame_memfd
	./$(BUILD_DIR)/test_t1_usb_cycle_guard
	./$(BUILD_DIR)/test_t1_touchbar_drm
	./$(BUILD_DIR)/test_t1_touchbar_session
	./$(BUILD_DIR)/test_t1_recovery

sanitize: sanitize-clean
	$(MAKE) CC=gcc BUILD_DIR=$(SANITIZER_DIR) CFLAGS='$(SANITIZER_CFLAGS)' \
		ASAN_OPTIONS='detect_leaks=1:halt_on_error=1' \
		UBSAN_OPTIONS='halt_on_error=1:print_stacktrace=1' test

sanitize-clean:
	rm -f $(SANITIZER_BINARIES)
	rmdir $(SANITIZER_DIR) 2>/dev/null || true

$(BUILD_DIR):
	mkdir -p $@

clean:
	rm -f $(TEST_BINARIES) $(SANITIZER_BINARIES)
	rmdir $(SANITIZER_DIR) 2>/dev/null || true
