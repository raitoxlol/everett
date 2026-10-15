import { cronJobs, makeFunctionReference } from "convex/server";

const crons = cronJobs();
crons.interval("expire undelivered ciphertext", { minutes: 1 }, makeFunctionReference<"mutation">("relay:expire"), {});
export default crons;
