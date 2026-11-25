import { h, render } from "preact";
import { api } from "../../src/api/client.ts";
import { NotificationSettings } from "../../src/components/settings/NotificationSettings.tsx";
import { startMediaCache } from "../../src/features/conversation/mediaController.ts";
import "../../src/style.css";

const key =
  "BLn9b-VR0ca83knDNZ32dCHGyjJp-1riX9ZTN40MqV8K_LpQmLqxC_DoHvqvFXO_nGdAB4W9dogZb_sM-uV4JbY";
const endpoint = "https://fcm.googleapis.com/fcm/send/test-";
let installed = false;
let subscription = null;
let subscribedFromClick = false;
const requests = [];
let subscriptionCount = 0;

api.pushSettings = async () => ({ public_key: key, installed });
PushManager.prototype.getSubscription = async () => subscription;
PushManager.prototype.subscribe = async (options) => {
  subscribedFromClick = navigator.userActivation.isActive;
  const value = options.applicationServerKey;
  if (!options.userVisibleOnly || value.length !== 65 || value[0] !== 4)
    throw new Error("Invalid subscription options");
  const address = endpoint + ++subscriptionCount;
  subscription = {
    options: { applicationServerKey: value.buffer },
    toJSON: () => ({ endpoint: address }),
    async unsubscribe() {
      subscription = null;
      return true;
    },
  };
  return subscription;
};
api.installPush = async (value) => {
  if (value.endpoint !== endpoint + subscriptionCount)
    throw new Error("Unexpected subscription");
  requests.push(`install-${subscriptionCount}`);
  installed = true;
  return { ok: true };
};
api.uninstallPush = async () => {
  requests.push("uninstall");
  installed = false;
  return { ok: true };
};

startMediaCache(new URL("/media-cache.js", location.href));
const mount = (key) =>
  render(
    h(NotificationSettings, {
      key,
      context: { rooms: [], private_access: {} },
    }),
    document.querySelector("#controls"),
  );
mount("initial");
window.expirePushSettings = () => {
  installed = false;
  mount("expired");
};
window.pushSettingsResult = async () => ({
  installed,
  subscribed: subscription !== null,
  requests,
  subscribedFromClick,
  worker: Boolean((await navigator.serviceWorker.getRegistration())?.active),
});
