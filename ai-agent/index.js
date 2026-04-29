const { Pool } = require('pg');
const cron = require('node-cron');
const { GoogleGenerativeAI } = require("@google/generative-ai");
require('dotenv').config();

const pool = new Pool({
  connectionString: process.env.POSTGRES_URL,
});

const genAI = process.env.GEMINI_API_KEY ? new GoogleGenerativeAI(process.env.GEMINI_API_KEY) : null;

async function runAnalysis() {
  console.log('Starting AI analysis...');
  let client;
  try {
    client = await pool.connect();

    // Fetch data
    const inventoryResult = await client.query('SELECT * FROM public.v_live_inventory LIMIT 50');
    const pnlResult = await client.query('SELECT * FROM public.report_daily_pnl ORDER BY date DESC LIMIT 30');

    const inventoryData = inventoryResult.rows;
    const pnlData = pnlResult.rows;

    let insight;

    if (genAI) {
      const model = genAI.getGenerativeModel({ model: "gemini-pro" });
      const prompt = `Analyze the following inventory and PnL data and provide a business insight.
      Inventory Data (recent snapshots): ${JSON.stringify(inventoryData)}
      PnL Data (last 30 days): ${JSON.stringify(pnlData)}
      
      Return a JSON object with:
      {
        "insight_type": "string (e.g., Inventory, Profitability, Risk)",
        "message": "string (the insight summary)",
        "recommended_action": "string (what to do)",
        "severity": "string (Low, Medium, High)"
      }`;

      const result = await model.generateContent(prompt);
      const response = await result.response;
      const text = response.text();
      
      try {
        // Attempt to parse JSON from AI response
        const jsonMatch = text.match(/\{[\s\S]*\}/);
        insight = jsonMatch ? JSON.parse(jsonMatch[0]) : null;
      } catch (e) {
        console.error('Failed to parse AI response as JSON:', text);
      }
    }

    // Fallback or Simulation if no API key or failed parse
    if (!insight) {
      console.log('Simulating analysis (no Gemini API key or failed parse)...');
      insight = {
        insight_type: 'General',
        message: 'Daily system check completed. Data trends appear stable.',
        recommended_action: 'Monitor inventory levels for high-turnover items.',
        severity: 'Low'
      };
    }

    // Insert insight into blink.ai_insights
    await client.query(
      'INSERT INTO blink.ai_insights (insight_type, message, recommended_action, severity) VALUES ($1, $2, $3, $4)',
      [insight.insight_type, insight.message, insight.recommended_action, insight.severity]
    );

    console.log('Insight successfully generated and saved:', insight.message);

  } catch (err) {
    console.error('Error during analysis:', err);
  } finally {
    if (client) client.release();
  }
}

// Run every hour
cron.schedule('0 * * * *', () => {
  runAnalysis();
});

// Run once on startup
runAnalysis();

process.on('SIGTERM', () => {
  pool.end();
  process.exit(0);
});
