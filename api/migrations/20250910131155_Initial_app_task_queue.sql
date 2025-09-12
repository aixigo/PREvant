CREATE TYPE task_status AS ENUM ('new', 'inProcess', 'done');

CREATE TABLE app_task (
   id UUID PRIMARY KEY,
   task JSONB NOT NULL,
   status task_status NOT NULL DEFAULT 'new',
   created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
   result_success JSONB,
   result_error JSONB,
);
