export interface User {
  remote_id: string;
  email: string;
  name: string;
  avatar: string | null;
  role: string;
  access_token: string;
  refresh_token: string;
  created_at: string;
  org_id: string | null;
  org_name: string | null;
}

export interface Project {
  remote_id: string;
  name: string;
  is_active: 0 | 1;
  created_at: string;
}

export interface TimerPayload {
  total_secs: number;
  is_running: boolean;
  project_id?: string;
}

export interface IdleUpdatePayload {
  idle_secs: number;
}
