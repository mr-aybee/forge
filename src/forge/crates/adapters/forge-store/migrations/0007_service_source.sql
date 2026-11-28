-- D-178: services applied from a Compose-format services file record the
-- file's name here, so a later apply removes only the services it manages.
-- Empty for services created in the console or through the API.
ALTER TABLE services ADD COLUMN source TEXT NOT NULL DEFAULT '';
