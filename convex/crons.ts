import { cronJobs, makeFunctionReference } from "convex/server";

const crons = cronJobs();
crons.interval("expire undelivered ciphertext", { minutes: 1 }, makeFunctionReference<"mutation">("relay:expire"), {});
crons.interval("expire agent ciphertext and rate counters", { minutes: 1 }, makeFunctionReference<"mutation">("agents:expire"), {});
export default crons;
