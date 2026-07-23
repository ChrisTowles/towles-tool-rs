import type { ComponentType } from "react";
import type { ScreenId } from "@/lib/screens";
import { AgentboardScreen } from "@/screens/agentboard";
import { BoardScreen } from "@/screens/board";
import { ClaudeSessionsScreen } from "@/screens/claude-sessions";
import { CockpitScreen } from "@/screens/cockpit";
import { DoctorScreen } from "@/screens/doctor";
import { GhPrsScreen } from "@/screens/gh-prs";
import { McpScreen } from "@/screens/mcp";
import { SettingsScreen } from "@/screens/settings";
import { SlackScreen } from "@/screens/slack";
import { TaskExplorerScreen } from "@/screens/task-explorer";
import { TelemetryScreen } from "@/screens/telemetry";

export const SCREEN_COMPONENTS: Record<ScreenId, ComponentType> = {
  cockpit: CockpitScreen,
  board: BoardScreen,
  agentboard: AgentboardScreen,
  slack: SlackScreen,
  doctor: DoctorScreen,
  "claude-sessions": ClaudeSessionsScreen,
  "gh-prs": GhPrsScreen,
  mcp: McpScreen,
  telemetry: TelemetryScreen,
  "task-explorer": TaskExplorerScreen,
  settings: SettingsScreen,
};
