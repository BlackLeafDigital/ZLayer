# IpAllocationResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**AllocatedCount** | **int32** | Number of allocated IPs | 
**AllocatedIps** | Pointer to **[]string** | List of allocated IP addresses (only included if requested) | [optional] 
**AvailableCount** | **int32** | Number of available IPs | 
**Cidr** | **string** | Overlay network CIDR | 
**TotalIps** | **int32** | Total available IPs in the range | 
**UtilizationPercent** | **float64** | Utilization percentage (0.0 - 100.0) | 

## Methods

### NewIpAllocationResponse

`func NewIpAllocationResponse(allocatedCount int32, availableCount int32, cidr string, totalIps int32, utilizationPercent float64, ) *IpAllocationResponse`

NewIpAllocationResponse instantiates a new IpAllocationResponse object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewIpAllocationResponseWithDefaults

`func NewIpAllocationResponseWithDefaults() *IpAllocationResponse`

NewIpAllocationResponseWithDefaults instantiates a new IpAllocationResponse object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetAllocatedCount

`func (o *IpAllocationResponse) GetAllocatedCount() int32`

GetAllocatedCount returns the AllocatedCount field if non-nil, zero value otherwise.

### GetAllocatedCountOk

`func (o *IpAllocationResponse) GetAllocatedCountOk() (*int32, bool)`

GetAllocatedCountOk returns a tuple with the AllocatedCount field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetAllocatedCount

`func (o *IpAllocationResponse) SetAllocatedCount(v int32)`

SetAllocatedCount sets AllocatedCount field to given value.


### GetAllocatedIps

`func (o *IpAllocationResponse) GetAllocatedIps() []string`

GetAllocatedIps returns the AllocatedIps field if non-nil, zero value otherwise.

### GetAllocatedIpsOk

`func (o *IpAllocationResponse) GetAllocatedIpsOk() (*[]string, bool)`

GetAllocatedIpsOk returns a tuple with the AllocatedIps field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetAllocatedIps

`func (o *IpAllocationResponse) SetAllocatedIps(v []string)`

SetAllocatedIps sets AllocatedIps field to given value.

### HasAllocatedIps

`func (o *IpAllocationResponse) HasAllocatedIps() bool`

HasAllocatedIps returns a boolean if a field has been set.

### SetAllocatedIpsNil

`func (o *IpAllocationResponse) SetAllocatedIpsNil(b bool)`

 SetAllocatedIpsNil sets the value for AllocatedIps to be an explicit nil

### UnsetAllocatedIps
`func (o *IpAllocationResponse) UnsetAllocatedIps()`

UnsetAllocatedIps ensures that no value is present for AllocatedIps, not even an explicit nil
### GetAvailableCount

`func (o *IpAllocationResponse) GetAvailableCount() int32`

GetAvailableCount returns the AvailableCount field if non-nil, zero value otherwise.

### GetAvailableCountOk

`func (o *IpAllocationResponse) GetAvailableCountOk() (*int32, bool)`

GetAvailableCountOk returns a tuple with the AvailableCount field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetAvailableCount

`func (o *IpAllocationResponse) SetAvailableCount(v int32)`

SetAvailableCount sets AvailableCount field to given value.


### GetCidr

`func (o *IpAllocationResponse) GetCidr() string`

GetCidr returns the Cidr field if non-nil, zero value otherwise.

### GetCidrOk

`func (o *IpAllocationResponse) GetCidrOk() (*string, bool)`

GetCidrOk returns a tuple with the Cidr field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCidr

`func (o *IpAllocationResponse) SetCidr(v string)`

SetCidr sets Cidr field to given value.


### GetTotalIps

`func (o *IpAllocationResponse) GetTotalIps() int32`

GetTotalIps returns the TotalIps field if non-nil, zero value otherwise.

### GetTotalIpsOk

`func (o *IpAllocationResponse) GetTotalIpsOk() (*int32, bool)`

GetTotalIpsOk returns a tuple with the TotalIps field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetTotalIps

`func (o *IpAllocationResponse) SetTotalIps(v int32)`

SetTotalIps sets TotalIps field to given value.


### GetUtilizationPercent

`func (o *IpAllocationResponse) GetUtilizationPercent() float64`

GetUtilizationPercent returns the UtilizationPercent field if non-nil, zero value otherwise.

### GetUtilizationPercentOk

`func (o *IpAllocationResponse) GetUtilizationPercentOk() (*float64, bool)`

GetUtilizationPercentOk returns a tuple with the UtilizationPercent field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetUtilizationPercent

`func (o *IpAllocationResponse) SetUtilizationPercent(v float64)`

SetUtilizationPercent sets UtilizationPercent field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


