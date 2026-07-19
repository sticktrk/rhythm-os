RHYTHM_PREBUILT_VERSION = local
RHYTHM_PREBUILT_SITE = $(BR2_EXTERNAL_RHYTHM_PATH)/../../../dist/bin/rpiz
RHYTHM_PREBUILT_SITE_METHOD = local
RHYTHM_PREBUILT_LICENSE = Apache-2.0

define RHYTHM_PREBUILT_INSTALL_TARGET_CMDS
	$(INSTALL) -D -m 0755 $(@D)/rhythm-server $(TARGET_DIR)/usr/bin/rhythm-server
	$(INSTALL) -D -m 0755 $(@D)/rhythm-chipd $(TARGET_DIR)/usr/bin/rhythm-chipd
	$(INSTALL) -D -m 0755 $(@D)/rhythm-host-recorder $(TARGET_DIR)/usr/bin/rhythm-host-recorder
endef

$(eval $(generic-package))
