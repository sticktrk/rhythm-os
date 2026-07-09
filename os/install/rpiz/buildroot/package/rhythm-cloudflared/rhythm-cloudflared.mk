################################################################################
#
# rhythm-cloudflared
#
################################################################################

RHYTHM_CLOUDFLARED_VERSION = 2026.6.0
RHYTHM_CLOUDFLARED_SITE = https://github.com/cloudflare/cloudflared/releases/download/$(RHYTHM_CLOUDFLARED_VERSION)
RHYTHM_CLOUDFLARED_SOURCE = cloudflared-linux-arm
RHYTHM_CLOUDFLARED_LICENSE = Apache-2.0

define RHYTHM_CLOUDFLARED_EXTRACT_CMDS
	cp $(RHYTHM_CLOUDFLARED_DL_DIR)/$(RHYTHM_CLOUDFLARED_SOURCE) $(@D)/cloudflared
endef

define RHYTHM_CLOUDFLARED_INSTALL_TARGET_CMDS
	$(INSTALL) -D -m 0755 $(@D)/cloudflared $(TARGET_DIR)/usr/bin/cloudflared
endef

$(eval $(generic-package))
