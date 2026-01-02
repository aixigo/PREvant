CREATE TABLE app_backup (
   app_name VARCHAR(512) PRIMARY KEY,
   app JSONB NOT NULL,
   infrastructure_payload JSONB NOT NULL,
   created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW()
);
