-- Runs once via docker-compose's db-init service.
CREATE DATABASE IF NOT EXISTS arnheid;
SET CLUSTER SETTING feature.vector_index.enabled = true;
