import { createContext } from "solid-js";
import { useContext } from "solid-js/types/server/reactive.js";

interface AppContextType {
  /**
   * The brand name of the application.
   * @example "Nativelink"
   */
  brand: string;
  /**
   * The address of nativelink server.
   * @example localhost:50051
   */
  address?: string;
  /**
   * Whether the application is connected to the server.
   */
  is_connected: boolean;
  /**
   * The timestamp of the last update from the server.
   * This is used to trigger UI updates when the connection status changes.
   * @example Date.now()
   */
  last_updated: number;
}

const AppContext = createContext<AppContextType>({
  brand: "Nativelink",
  is_connected: false,
  last_updated: 0,
});

export const useAppContext = () => useContext(AppContext);
