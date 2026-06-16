ALTER TABLE hubs DROP CONSTRAINT hubs_type_check;
ALTER TABLE hubs ADD CONSTRAINT hubs_type_check CHECK (type IN ('homeAssistant', 'hue', 'esp32', 'server'));
