import { cronJobs, makeFunctionReference } from "convex/server";

const crons = cronJobs();
crons.interval("expire undelivered ciphertext", { minutes: 1 }, makeFunctionReference<"mutation">("relay:expire"), {});
crons.interval("expire device enrollments", { minutes: 1 }, makeFunctionReference<"mutation">("hosted:expire"), {});
export default crons;
