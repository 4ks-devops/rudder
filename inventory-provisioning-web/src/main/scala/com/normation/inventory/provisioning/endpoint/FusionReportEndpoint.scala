/*
*************************************************************************************
* Copyright 2011 Normation SAS
*************************************************************************************
*
* This file is part of Rudder.
*
* Rudder is free software: you can redistribute it and/or modify
* it under the terms of the GNU General Public License as published by
* the Free Software Foundation, either version 3 of the License, or
* (at your option) any later version.
*
* In accordance with the terms of section 7 (7. Additional Terms.) of
* the GNU General Public License version 3, the copyright holders add
* the following Additional permissions:
* Notwithstanding to the terms of section 5 (5. Conveying Modified Source
* Versions) and 6 (6. Conveying Non-Source Forms.) of the GNU General
* Public License version 3, when you create a Related Module, this
* Related Module is not considered as a part of the work and may be
* distributed under the license agreement of your choice.
* A "Related Module" means a set of sources files including their
* documentation that, without modification of the Source Code, enables
* supplementary functions or services in addition to those offered by
* the Software.
*
* Rudder is distributed in the hope that it will be useful,
* but WITHOUT ANY WARRANTY; without even the implied warranty of
* MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
* GNU General Public License for more details.
*
* You should have received a copy of the GNU General Public License
* along with Rudder.  If not, see <http://www.gnu.org/licenses/>.

*
*************************************************************************************
*/

package com.normation.inventory.provisioning.endpoint


import org.springframework.stereotype.Controller
import org.springframework.web.bind.annotation.RequestMapping
import org.springframework.web.bind.annotation.RequestMethod
import org.springframework.web.multipart.MultipartFile
import org.springframework.web.multipart.support.DefaultMultipartHttpServletRequest
import org.springframework.http.{HttpStatus,ResponseEntity}
import com.normation.inventory.domain._
import com.normation.inventory.services.provisioning._
import net.liftweb.common._
import scala.collection.JavaConversions._
import org.joda.time.Duration
import org.joda.time.format.PeriodFormat
import org.slf4j.LoggerFactory
import java.io.{IOException, File, FileInputStream, InputStream, FileOutputStream}
import FusionReportEndpoint._
import com.unboundid.ldif.LDIFChangeRecord
import javax.servlet.http.HttpServletRequest
import com.normation.inventory.services.core.FullInventoryRepository
import org.springframework.util.MultiValueMap
import org.springframework.util.CollectionUtils.MultiValueMapAdapter
import org.springframework.http.HttpHeaders
import scala.util.control.NonFatal
import com.normation.ldap.sdk.LDAPConnectionProvider
import com.normation.ldap.sdk.RwLDAPConnection
import com.normation.inventory.ldap.core.InventoryDit

object FusionReportEndpoint{
  val printer = PeriodFormat.getDefault
}

//messages to know if the backend accepted to process the report
final case object OkToSave
final case object TooManyInQueue
final case class QueueInfo(max: Int, current: Int)
final case object GetQueueInfo


@Controller
class FusionReportEndpoint(
    unmarshaller:ReportUnmarshaller
  , reportSaver:ReportSaver[Seq[LDIFChangeRecord]]
  , queueSize: Int
  , repo : FullInventoryRepository[Seq[LDIFChangeRecord]]
  , digestService : InventoryDigestServiceV1
  , ldap            : LDAPConnectionProvider[RwLDAPConnection]
  , nodeInventoryDit: InventoryDit
) extends Loggable {

  //start the report processor actor
  ReportProcessor.start


  /**
   * A status URL that just allow to check
   * that the endpoint is alive
   */
  @RequestMapping(
    value = Array("/api/status"),
    method = Array(RequestMethod.GET)
  )
  def checkStatus() = new ResponseEntity("OK", HttpStatus.OK)


  /**
   * Info on current number of elements in queue
   */
  @RequestMapping(
    value = Array("/api/info"),
    method = Array(RequestMethod.GET)
  )
  def queueInfo() = {
    (ReportProcessor !? GetQueueInfo) match {
      case QueueInfo(max, current) =>
        val saturated = (current+1) >= max
        val code = if(saturated) HttpStatus.TOO_MANY_REQUESTS else HttpStatus.OK
        val json = s"""{"queueMaxSize":$max, "queueFillCount":$current, "queueSaturated":$saturated}"""
        val headers = new HttpHeaders()
        headers.add("content-type", "application/json")
        new ResponseEntity(json, headers, code)
      case x =>
        new ResponseEntity(s"Internal error: the queue info query answered with the unknown message: '${x}'", HttpStatus.INTERNAL_SERVER_ERROR)
    }
  }

  /**
   * The actual endpoint. It's here that
   * upload requests arrive
   * @param request
   * @return
   */
  @RequestMapping(
    value = Array("/upload"),
    method = Array(RequestMethod.POST)
  )
  def onSubmit(request: HttpServletRequest) = {
    def defaultBadAnswer(reason: String) = {
      new ResponseEntity(
          s"""${reason}.
              |You have to POST a request with exactly one file in attachment (with 'content-disposition': file) with parameter name 'file'
              |For example, for curl, use: curl -F "file=@path/to/file"
          |""".stripMargin
        , HttpStatus.PRECONDITION_FAILED
      )
    }

    /*
     * Before actually trying to save, check that LDAP is up to at least
     * avoid the case where we are telling the use "everything is fine"
     * but just fail after.
     */
    def save(reportName: String, report: InventoryReport): ResponseEntity[String] = {
      checkLdapAlive match {
        case Full(ok) =>
          (ReportProcessor !? report) match {
          case OkToSave =>
            //release connection
            new ResponseEntity("Inventory correctly received and sent to inventory processor.\n", HttpStatus.ACCEPTED)
          case TooManyInQueue =>
            new ResponseEntity("Too many inventories waiting to be saved.\n", HttpStatus.SERVICE_UNAVAILABLE)
          }
        case eb: EmptyBox =>
          val e = (eb ?~! s"There is an error with the LDAP backend preventing acceptation of inventory '${reportName}'")
          logger.error(e.messageChain)
          new ResponseEntity(e.messageChain, HttpStatus.INTERNAL_SERVER_ERROR)
      }
    }

    /*
     * A method that check LDAP health status.
     * It must be quick and simple.
     */
    def checkLdapAlive: Box[String] = {
      for {
        con <- ldap
        res <- con.get(nodeInventoryDit.NODES.dn, "1.1")
      } yield {
        "ok"
      }
    }

    def parseInventory(inventoryFile : MultipartFile, signatureFile : Option[MultipartFile]) = {
      val inventory = inventoryFile.getOriginalFilename()
      //copy the session file somewhere where it won't be deleted on that method return
      logger.info(s"New input inventory: '${inventory}'")
      logger.trace(s"Start post parsing inventory '${inventory}'")
      try {
        val in = inventoryFile.getInputStream
        val start = System.currentTimeMillis

        (unmarshaller.fromXml(inventoryFile.getName,in) ?~! "Can't parse the input inventory, aborting") match {
          case Full(report) =>
            val afterParsing = System.currentTimeMillis
            logger.info(s"Inventory '${inventory}' parsed in ${printer.print(new Duration(start, afterParsing).toPeriod)} ms, now checking signature")
            // Do we have a signature ?
            signatureFile match {
              // Signature here, check it
              case Some(sig) => {
                val signatureStream = sig.getInputStream()
                val inventoryStream = inventoryFile.getInputStream()
                val response = for {
                  digest       <- digestService.parse(signatureStream)
                  (key,_) <- digestService.getKey(report)
                  checked      <- digestService.check(key, digest, inventoryFile.getInputStream)
                  } yield {
                    if (checked) {
                      // Signature is valid, send it to save engine
                      logger.info(s"Inventory '${inventory}' signature checked in ${printer.print(new Duration(afterParsing, System.currentTimeMillis).toPeriod)} ms, now saving")
                      // Set the keyStatus to Certified
                      // For now we set the status to certified since we want pending inventories to have their inventory signed
                      // When we will have a 'pending' status for keys we should set that value instead of certified
                      val certifiedReport = report.copy(node = report.node.copyWithMain(main => main.copy(keyStatus = CertifiedKey)))
                      save(inventory, certifiedReport)
                    } else {
                      // Signature is not valid, reject inventory
                      val msg = s"Rejecting Inventory '${inventory}' for Node '${report.node.main.id.value}' because signature is not valid, you can update the inventory key by running the following command '/opt/rudder/bin/rudder-keys change-key ${report.node.main.id.value} <your new public key>'"
                      logger.error(msg)
                      new ResponseEntity(msg, HttpStatus.UNAUTHORIZED)
                    }
                  }
                signatureStream.close()
                inventoryStream.close()
                response match {
                  case Full(response) =>
                    // Response of signature checking is ok send it
                    response
                  case eb: EmptyBox =>
                    // An error occurred while checking signature
                    logger.error(eb)
                    val fail = eb ?~! "Error when trying to check inventory signature"
                    logger.error(fail.messageChain)
                    logger.debug(s"Time to error: ${printer.print(new Duration(start, System.currentTimeMillis).toPeriod)} ms")
                    new ResponseEntity(fail.messageChain, HttpStatus.PRECONDITION_FAILED)
                }
              }

              // There is no Signature
              case None =>
                // Check if we need a signature or not
                digestService.getKey(report) match {
                  // Status is undefined => We accept unsigned inventory
                  case Full((_,UndefinedKey)) => {
                    save(inventory, report)
                  }
                  // We are in certified state, refuse inventory with no signature
                  case Full((_,CertifiedKey))  =>
                    val msg = s"Reject inventory '${inventory}' for Node '${report.node.main.id.value}' because signature is missing, you can go back to unsigned state by running the following command '/opt/rudder/bin/rudder-keys reset-status ${report.node.main.id.value}'"
                    logger.error(msg)
                    new ResponseEntity(msg, HttpStatus.UNAUTHORIZED)
                  // An error occurred while checking inventory key status
                  case eb: EmptyBox =>
                    logger.error(eb)
                    val fail = eb ?~! "Error when trying to check inventory key status"
                    logger.error(fail.messageChain)
                    logger.debug(s"Time to error: ${printer.print(new Duration(start, System.currentTimeMillis).toPeriod)} ms")
                    new ResponseEntity(fail.messageChain, HttpStatus.PRECONDITION_FAILED)
                }
            }

          // Error during parsing
          case eb : EmptyBox =>
            val fail = eb ?~! "Error when trying to parse inventory"
            logger.error(fail.messageChain)
            fail.rootExceptionCause.foreach { exp => logger.error(s"Exception was: ${exp}") }
            logger.debug(s"Time to error: ${printer.print(new Duration(start, System.currentTimeMillis).toPeriod)} ms")
            new ResponseEntity(fail.messageChain, HttpStatus.PRECONDITION_FAILED)
        }
      } catch {
        case NonFatal(ex) =>
          val msg = s"Error when trying to parse inventory '${inventory}': ${ex.getMessage}"
          logger.error(msg, ex)
          new ResponseEntity(msg, HttpStatus.PRECONDITION_FAILED)
      }
    }

    request match {
      case multipart: DefaultMultipartHttpServletRequest =>
        import scala.collection.JavaConversions._
        val params : Map[String,MultipartFile]= multipart.getFileMap.toMap

        /*
         * params are :
         * * 'file' => The inventory file
         * * 'signature' => The signature file
         */
        val inventoryParam = "file"
        val signatureParam = "signature"

        (params.get(inventoryParam),params.get(signatureParam)) match {
          // No inventory, error
          case (None,_) =>
            defaultBadAnswer("No inventory sent")
          // Only inventory sent
          case (Some(inventory),None) => {
            parseInventory(inventory,None)
          }
          // An inventory and signature check them!
          case (Some(inventory), Some(signature)) => {
            parseInventory(inventory,Some(signature))
          }
        }
    }
  }

  import scala.actors.Actor
  import Actor._

  /**
   * An asynchronous actor process the query
   */
  private object ReportProcessor extends Actor {
    self =>

    //a message from the processor to self
    //saying it finish processing
    case object Processed

    var inQueue = 0

    //that actor only check the number of queued elements and decide to
    //queue it or not
    override def act = {
      loop { react {
          case GetQueueInfo =>
            reply(QueueInfo(queueSize, inQueue))

          case i:InventoryReport =>

            if(inQueue < queueSize) {
              actualProcessor ! i
              reply(OkToSave)
              inQueue += 1
            } else {
              logger.warn(s"Not processing inventory ${i.name} because there is already the maximum number (${inQueue}) of inventory waiting to be processed")
              reply(TooManyInQueue)
            }

          case Processed =>
            inQueue -= 1
      } }
    }

    //this is the actor that process the report
    val actualProcessor = new Actor {
      override def act = {
        loop { react {
            case i:InventoryReport =>
              saveReport(i)
              self ! Processed
        } }
      }
    }

    actualProcessor.start

  }


  private def saveReport(report:InventoryReport) : Unit = {
    logger.trace("Start post processing of report %s".format(report.name))
    try {
      val start = System.currentTimeMillis
      (reportSaver.save(report) ?~! "Can't merge inventory report in LDAP directory, aborting") match {
        case Empty => logger.error("The report is empty, not saving anything")
        case f:Failure =>
          logger.error("Error when trying to process report: %s".format(f.messageChain),f)
        case Full(report) =>
          logger.debug("Report saved.")
      }
      logger.info("Report %s processed in %s ms".format(report.name, printer.print(new Duration(start, System.currentTimeMillis).toPeriod)))
    } catch {
      case e:Exception =>
        logger.error("Exception when processing report %s".format(report.name))
        logger.error("Reported exception is: ", e)
    }
  }
}
