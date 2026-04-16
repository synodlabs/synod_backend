import fs from 'fs';

async function run() {
  try {
    const res = await fetch("http://127.0.0.1:8080/v1/dashboard");
    const dashData = await res.json();
    console.log("DASHBOARD DATA:", dashData);
    
    if (dashData && dashData.length > 0) {
      const treasuryId = dashData[0].treasury_id;
      const res2 = await fetch(`http://127.0.0.1:8080/v1/agents/${treasuryId}`);
      const agents = await res2.json();
      console.log("AGENTS DATA:", JSON.stringify(agents, null, 2));
    }
  } catch (err) {
    console.error(err);
  }
}
run();
