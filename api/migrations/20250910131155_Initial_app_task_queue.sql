CREATE TYPE task_status AS ENUM ('queued', 'running', 'done');

CREATE TABLE app_task (
   id UUID PRIMARY KEY,
   app_name VARCHAR(512) NOT NULL,
   task JSONB NOT NULL,
   status task_status NOT NULL DEFAULT 'queued',
   created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
   result_success JSONB,
   result_error JSONB
);
