export interface User {
  id: number;
  remote_id: string;
  email: string;
  name: string;
  avatar: string | null;
  role: string;
  access_token: string;
  refresh_token: string;
  created_at: string;
}

export interface Project {
  id: number;
  remote_id: string;
  name: string;
  is_active: number;
  created_at: string;
}

export interface TimerPayload {
  total_secs: number;
  is_running: boolean;
}
