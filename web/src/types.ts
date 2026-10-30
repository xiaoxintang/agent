export interface Agent {
  id: string;
  name: string;
  url: string;
}
export interface Session {
  username: string;
  version: string;
}
export interface Records {
  agents: Agent[];
  users: any[];
  assignments: any[];
  tasks: any[];
}
